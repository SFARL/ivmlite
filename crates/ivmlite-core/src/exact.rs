//! M2c spec §4: the exact sum of the inputs classified as non-integer — a
//! fixed-point superaccumulator covering every finite `f64` (2^-1074 to
//! 2^1024) with headroom for 2^64 terms. Adding and subtracting are exact, so
//! the state depends only on the multiset of live inputs. Written by hand: the
//! project takes no new dependency for it.

/// 64-bit limbs, least significant first, two's complement: 2,240 bits, the
/// spec's "about 2,200 bits" (M2c spec §4) rounded up to whole limbs.
pub const EXACT_LIMBS: usize = 35;
/// The stored size: big-endian, most significant limb first.
pub const EXACT_BYTES: usize = EXACT_LIMBS * 8;
/// The least significant bit is worth 2^-1074, the smallest subnormal.
const LSB_SCALE: u32 = 1074;
/// Every reachable sum is below 2^2225 units in magnitude: one term is below
/// 2^1024 * 2^63 = 2^1087, 2^64 terms stay below 2^1151, and the 1074
/// fractional bits make that 2^2225 units. Bits from here up copy the sign.
const VALUE_BITS: u32 = 2225;
const _: () = assert!(VALUE_BITS / 64 == EXACT_LIMBS as u32 - 1);

/// The `f64` format: 52 stored fraction bits, 53 significant bits.
const FRACTION_BITS: u32 = 52;
const SIGNIFICAND_BITS: u32 = FRACTION_BITS + 1;
const FRACTION_MASK: u64 = (1 << FRACTION_BITS) - 1;
const EXPONENT_MASK: u64 = 0x7ff;
const EXPONENT_BIAS: i64 = 1023;
/// The exponent of the least significant bit of a subnormal.
const SUBNORMAL_LSB_EXPONENT: i64 = -1074;

/// Extra fractional bits the mean's quotient carries, so that even a
/// subnormal mean has its guard bit inside the quotient.
const MEAN_EXTRA_BITS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactSum {
    limbs: [u64; EXACT_LIMBS],
}

impl ExactSum {
    pub fn zero() -> ExactSum {
        ExactSum {
            limbs: [0; EXACT_LIMBS],
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Add `x * weight` exactly. `x` must be finite: the aggregate counts
    /// infinities itself (M2c spec §4, amendment 5).
    pub fn add(&mut self, x: f64, weight: i64) {
        assert!(x.is_finite(), "ExactSum holds finite values only");
        if x == 0.0 || weight == 0 {
            return;
        }
        let bits = x.to_bits();
        let exponent = ((bits >> FRACTION_BITS) & EXPONENT_MASK) as u32;
        let fraction = bits & FRACTION_MASK;
        // x = mantissa * 2^(shift - 1074).
        let (mantissa, shift) = if exponent == 0 {
            (fraction, 0)
        } else {
            (fraction | (1 << FRACTION_BITS), exponent - 1)
        };
        // < 2^53 * 2^63: fits a u128.
        let magnitude = u128::from(mantissa) * u128::from(weight.unsigned_abs());
        let negative = (bits >> 63 == 1) != (weight < 0);
        self.add_shifted(magnitude, shift, negative);
    }

    /// Add the integer `n` exactly.
    pub fn add_int(&mut self, n: i128) {
        if n != 0 {
            self.add_shifted(n.unsigned_abs(), LSB_SCALE, n < 0);
        }
    }

    /// Add `other` exactly (two's-complement addition, modulo 2^2240).
    pub fn add_sum(&mut self, other: &ExactSum) {
        let mut carry = false;
        for (a, &b) in self.limbs.iter_mut().zip(&other.limbs) {
            let (s1, c1) = a.overflowing_add(b);
            let (s2, c2) = s1.overflowing_add(u64::from(carry));
            *a = s2;
            carry = c1 || c2;
        }
    }

    /// `magnitude * 2^shift` units, added or — `negative` — subtracted.
    /// `shift` is at most 2045 (the largest finite exponent), so the three
    /// parts end at limb 33 and a carry or borrow stays inside the limbs.
    fn add_shifted(&mut self, magnitude: u128, shift: u32, negative: bool) {
        let parts = shifted_parts(magnitude, shift % 64);
        let first = (shift / 64) as usize;
        if negative {
            self.sub_parts(first, parts);
        } else {
            self.add_parts(first, parts);
        }
    }

    fn add_parts(&mut self, first: usize, parts: [u64; 3]) {
        let mut carry = false;
        for (i, limb) in self.limbs.iter_mut().enumerate().skip(first) {
            let part = parts.get(i - first).copied().unwrap_or(0);
            if part == 0 && !carry && i >= first + parts.len() {
                break;
            }
            let (s1, c1) = limb.overflowing_add(part);
            let (s2, c2) = s1.overflowing_add(u64::from(carry));
            *limb = s2;
            carry = c1 || c2;
        }
    }

    fn sub_parts(&mut self, first: usize, parts: [u64; 3]) {
        let mut borrow = false;
        for (i, limb) in self.limbs.iter_mut().enumerate().skip(first) {
            let part = parts.get(i - first).copied().unwrap_or(0);
            if part == 0 && !borrow && i >= first + parts.len() {
                break;
            }
            let (d1, b1) = limb.overflowing_sub(part);
            let (d2, b2) = d1.overflowing_sub(u64::from(borrow));
            *limb = d2;
            borrow = b1 || b2;
        }
    }

    fn is_negative(&self) -> bool {
        self.limbs[EXACT_LIMBS - 1] >> 63 == 1
    }

    /// |sum|, as unsigned limbs.
    fn magnitude(&self) -> [u64; EXACT_LIMBS] {
        let mut limbs = self.limbs;
        if self.is_negative() {
            let mut carry = true;
            for limb in &mut limbs {
                let (v, c) = (!*limb).overflowing_add(u64::from(carry));
                *limb = v;
                carry = c;
            }
        }
        limbs
    }

    fn with_sign(&self, magnitude: f64) -> f64 {
        if self.is_negative() {
            -magnitude
        } else {
            magnitude
        }
    }

    /// The sum, rounded once to the nearest `f64`, ties to even (M2c spec §4);
    /// ±Inf beyond the `f64` range, and +0.0 for an exact zero.
    pub fn round(&self) -> f64 {
        self.with_sign(round_magnitude(&self.magnitude(), LSB_SCALE, false))
    }

    /// The sum divided by `divisor` (> 0), by exact long division, rounded
    /// once to nearest, ties to even (M2c spec §4, AVG). The remainder takes
    /// part in the rounding as a sticky bit. A negative quotient that rounds
    /// to zero is -0.0, as IEEE 754 division gives.
    pub fn div_round(&self, divisor: i64) -> f64 {
        assert!(divisor > 0, "a mean divides by a positive count");
        let mut limbs = self.magnitude();
        // |sum| < 2^2225 units leaves room for the extra bits.
        for i in (1..EXACT_LIMBS).rev() {
            limbs[i] = (limbs[i] << MEAN_EXTRA_BITS) | (limbs[i - 1] >> (64 - MEAN_EXTRA_BITS));
        }
        limbs[0] <<= MEAN_EXTRA_BITS;
        let d = u128::from(divisor.unsigned_abs());
        let mut remainder: u128 = 0;
        for limb in limbs.iter_mut().rev() {
            let current = (remainder << 64) | u128::from(*limb);
            *limb = (current / d) as u64;
            remainder = current % d;
        }
        let scale = LSB_SCALE + MEAN_EXTRA_BITS;
        self.with_sign(round_magnitude(&limbs, scale, remainder != 0))
    }

    pub fn to_be_bytes(&self) -> [u8; EXACT_BYTES] {
        let mut out = [0u8; EXACT_BYTES];
        for (chunk, limb) in out.chunks_exact_mut(8).zip(self.limbs.iter().rev()) {
            chunk.copy_from_slice(&limb.to_be_bytes());
        }
        out
    }

    /// The inverse of `to_be_bytes`. `None` for the wrong length, or for a
    /// value no sum of finite terms can reach (stored state was corrupted).
    pub fn from_be_bytes(bytes: &[u8]) -> Option<ExactSum> {
        if bytes.len() != EXACT_BYTES {
            return None;
        }
        let mut limbs = [0u64; EXACT_LIMBS];
        for (limb, chunk) in limbs.iter_mut().rev().zip(bytes.chunks_exact(8)) {
            *limb = u64::from_be_bytes(chunk.try_into().expect("chunks of 8"));
        }
        let high = limbs[EXACT_LIMBS - 1] >> (VALUE_BITS % 64);
        let all_ones = u64::MAX >> (VALUE_BITS % 64);
        if high != 0 && high != all_ones {
            return None;
        }
        Some(ExactSum { limbs })
    }
}

/// `magnitude << offset` (offset < 64) as three limbs, low first.
fn shifted_parts(magnitude: u128, offset: u32) -> [u64; 3] {
    let lo = magnitude as u64;
    let hi = (magnitude >> 64) as u64;
    if offset == 0 {
        return [lo, hi, 0];
    }
    [
        lo << offset,
        (lo >> (64 - offset)) | (hi << offset),
        hi >> (64 - offset),
    ]
}

fn highest_bit(limbs: &[u64; EXACT_LIMBS]) -> Option<u32> {
    limbs
        .iter()
        .enumerate()
        .rev()
        .find(|(_, &l)| l != 0)
        .map(|(i, &l)| i as u32 * 64 + 63 - l.leading_zeros())
}

fn bit(limbs: &[u64; EXACT_LIMBS], pos: u32) -> bool {
    (limbs[(pos / 64) as usize] >> (pos % 64)) & 1 == 1
}

/// The 53 bits starting at `start`.
fn significand_at(limbs: &[u64; EXACT_LIMBS], start: u32) -> u64 {
    let (i, off) = ((start / 64) as usize, start % 64);
    let lo = limbs[i] >> off;
    let hi = if off == 0 || i + 1 >= EXACT_LIMBS {
        0
    } else {
        limbs[i + 1] << (64 - off)
    };
    (lo | hi) & ((1 << SIGNIFICAND_BITS) - 1)
}

/// Whether any bit below `pos` is set.
fn any_below(limbs: &[u64; EXACT_LIMBS], pos: u32) -> bool {
    let (i, off) = ((pos / 64) as usize, pos % 64);
    limbs[..i].iter().any(|&l| l != 0) || (off > 0 && limbs[i] & ((1 << off) - 1) != 0)
}

/// `limbs * 2^-scale` (plus a sticky fraction below it), rounded once to the
/// nearest `f64`, ties to even (M2c spec §4). Non-negative.
fn round_magnitude(limbs: &[u64; EXACT_LIMBS], scale: u32, sticky_in: bool) -> f64 {
    let Some(top) = highest_bit(limbs) else {
        return 0.0;
    };
    // The position of the result's least significant bit: 53 significant
    // bits, never finer than 2^-1074 (M2c spec §4: every finite f64,
    // subnormals included).
    let lowest_kept = i64::from(scale) + SUBNORMAL_LSB_EXPONENT;
    let drop = (i64::from(top) - i64::from(FRACTION_BITS)).max(lowest_kept);
    if drop <= 0 {
        // At most 53 bits at a multiple of 2^-1074: exact (only at scale 1074).
        return compose(limbs[0], SUBNORMAL_LSB_EXPONENT);
    }
    let drop = drop as u32;
    let mut q = significand_at(limbs, drop);
    let guard = bit(limbs, drop - 1);
    let sticky = sticky_in || any_below(limbs, drop - 1);
    let mut exponent = i64::from(drop) - i64::from(scale);
    if guard && (sticky || q & 1 == 1) {
        q += 1;
        if q == 1 << SIGNIFICAND_BITS {
            q >>= 1;
            exponent += 1;
        }
    }
    compose(q, exponent)
}

/// `q * 2^exponent` as an `f64`; `q < 2^53`, and `q < 2^52` only at the
/// subnormal exponent -1074. Infinity when the value is 2^1024 or more.
fn compose(q: u64, exponent: i64) -> f64 {
    if q < 1 << FRACTION_BITS {
        debug_assert_eq!(
            exponent, SUBNORMAL_LSB_EXPONENT,
            "a short significand is subnormal"
        );
        return f64::from_bits(q);
    }
    let biased = exponent + i64::from(FRACTION_BITS) + EXPONENT_BIAS;
    if biased >= EXPONENT_MASK as i64 {
        return f64::INFINITY;
    }
    f64::from_bits(((biased as u64) << FRACTION_BITS) | (q & FRACTION_MASK))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SplitMix64;

    /// A random dyadic term `m * 2^k`, |m| < 2^40, k in [-30, 30], and a weight.
    fn term(rng: &mut SplitMix64) -> (i64, i32, i64) {
        let m = rng.below(1 << 40) as i64 - (1 << 39);
        let k = rng.below(61) as i32 - 30;
        let w = rng.below(7) as i64 - 3;
        (m, k, w)
    }

    fn value((m, k, _): (i64, i32, i64)) -> f64 {
        m as f64 * 2f64.powi(k)
    }

    /// The reference built from i128 pieces (spec §6): every term is a whole
    /// number of 2^-30 units below 2^71, so the exact sum fits an i128, and
    /// `i128 as f64` rounds to nearest, ties to even.
    fn reference(terms: &[(i64, i32, i64)]) -> f64 {
        let units: i128 = terms
            .iter()
            .map(|&(m, k, w)| (i128::from(m) << (k + 30)) * i128::from(w))
            .sum();
        units as f64 * 2f64.powi(-30)
    }

    fn sum_of(terms: &[(i64, i32, i64)]) -> ExactSum {
        let mut s = ExactSum::zero();
        for &t in terms {
            s.add(value(t), t.2);
        }
        s
    }

    #[test]
    fn rounding_matches_an_i128_reference() {
        let mut rng = SplitMix64(1);
        for _ in 0..2000 {
            let n = 1 + rng.below(40) as usize;
            let terms: Vec<_> = (0..n).map(|_| term(&mut rng)).collect();
            let got = sum_of(&terms).round();
            assert_eq!(got.to_bits(), reference(&terms).to_bits(), "{terms:?}");
        }
    }

    #[test]
    fn add_then_sub_is_the_identity() {
        let mut rng = SplitMix64(2);
        let base: Vec<_> = (0..20).map(|_| term(&mut rng)).collect();
        let before = sum_of(&base);
        let mut s = before.clone();
        let extra: Vec<f64> = vec![0.1, -3.75e300, 5e-324, f64::MAX, -f64::MIN_POSITIVE];
        for &x in &extra {
            s.add(x, 3);
        }
        for &x in extra.iter().rev() {
            s.add(x, -3);
        }
        assert_eq!(s, before);
    }

    #[test]
    fn the_result_does_not_depend_on_the_order() {
        let mut rng = SplitMix64(3);
        let mut terms: Vec<_> = (0..50).map(|_| term(&mut rng)).collect();
        let forward = sum_of(&terms);
        terms.reverse();
        assert_eq!(sum_of(&terms), forward);
        terms.swap(3, 40);
        assert_eq!(sum_of(&terms), forward);
    }

    #[test]
    fn cancellation_reaches_exact_zero() {
        let mut s = ExactSum::zero();
        s.add(1e300, 1);
        s.add(1.0, 1);
        s.add(-1e300, 1);
        s.add(-1.0, 1);
        assert!(s.is_zero());
        assert_eq!(
            s.round().to_bits(),
            0.0f64.to_bits(),
            "an exact zero rounds to +0.0"
        );
    }

    #[test]
    fn subnormal_sums_are_exact() {
        let tiny = f64::from_bits(1); // 2^-1074
        let mut s = ExactSum::zero();
        s.add(tiny, 3);
        assert_eq!(s.round(), f64::from_bits(3));
        s.add(f64::MIN_POSITIVE, 1); // 2^-1022, the smallest normal
        assert_eq!(s.round(), f64::MIN_POSITIVE + f64::from_bits(3));
        let mut t = ExactSum::zero();
        t.add(tiny, -1);
        assert_eq!(t.round(), -tiny);
    }

    #[test]
    fn max_finite_and_infinity() {
        let mut s = ExactSum::zero();
        s.add(f64::MAX, 1);
        assert_eq!(s.round(), f64::MAX);
        s.add(f64::MAX, 1);
        assert_eq!(s.round(), f64::INFINITY);
        s.add(f64::MAX, -1);
        assert_eq!(s.round(), f64::MAX, "exact, so the overflow is undone");
        s.add(f64::MAX, -2);
        assert_eq!(s.round(), -f64::MAX);
    }

    #[test]
    fn two_pow_60_equal_terms() {
        let mut s = ExactSum::zero();
        s.add(1.5, 1 << 60);
        assert_eq!(s.round(), 1.5 * 2f64.powi(60));
        s.add(f64::MAX, 1 << 60);
        assert_eq!(s.round(), f64::INFINITY);
        s.add(f64::MAX, -(1 << 60));
        s.add(1.5, -(1 << 60));
        assert!(s.is_zero());
    }

    #[test]
    fn ties_round_to_even() {
        // 2^53 + 1 is exactly half-way between 2^53 and 2^53 + 2: to even, 2^53.
        let mut s = ExactSum::zero();
        s.add(2f64.powi(53), 1);
        s.add(1.0, 1);
        assert_eq!(s.round(), 2f64.powi(53));
        // 2^53 + 3 is half-way between 2^53 + 2 and 2^53 + 4: to even, 2^53 + 4.
        s.add(2.0, 1);
        assert_eq!(s.round(), 2f64.powi(53) + 4.0);
        // Just above half-way rounds up: 2^53 + 1 + 2^-60.
        let mut t = ExactSum::zero();
        t.add(2f64.powi(53), 1);
        t.add(1.0, 1);
        t.add(2f64.powi(-60), 1);
        assert_eq!(t.round(), 2f64.powi(53) + 2.0);
    }

    #[test]
    fn add_int_is_exact_across_the_i128_range() {
        let mut s = ExactSum::zero();
        s.add_int(i128::from(i64::MAX) * 4);
        s.add_int(-(i128::from(i64::MAX) * 4));
        assert!(s.is_zero());
        s.add_int(9_007_199_254_740_993); // 2^53 + 1
        assert_eq!(s.round(), 9_007_199_254_740_992.0);
        s.add(0.5, 1);
        assert_eq!(s.round(), 9_007_199_254_740_994.0, "2^53 + 1.5 rounds up");
    }

    #[test]
    fn mean_is_correctly_rounded_against_an_i128_reference() {
        let mut rng = SplitMix64(4);
        for _ in 0..2000 {
            // Positive terms with |m| in [2^39, 2^40), so the quotient keeps
            // more than 64 bits after the reference's shift.
            let n = 1 + rng.below(30) as usize;
            let terms: Vec<_> = (0..n)
                .map(|_| {
                    (
                        (1 << 39) + rng.below(1 << 39) as i64,
                        rng.below(41) as i32 - 20,
                        1,
                    )
                })
                .collect();
            let units: i128 = terms
                .iter()
                .map(|&(m, k, _)| i128::from(m) << (k + 30))
                .sum();
            let count = 1 + rng.below(1000) as i64;
            let want = mean_reference(units, count);
            assert_eq!(
                sum_of(&terms).div_round(count).to_bits(),
                want.to_bits(),
                "{terms:?} / {count}"
            );
        }
    }

    /// `units * 2^-30 / count`, correctly rounded: shift the numerator up so
    /// the quotient has > 64 bits, keep the remainder as one sticky bit below
    /// it, and let `u128 as f64` round to nearest, ties to even.
    fn mean_reference(units: i128, count: i64) -> f64 {
        let magnitude = units.unsigned_abs();
        let shift = magnitude.leading_zeros() - 1;
        let shifted = magnitude << shift;
        let q = shifted / count as u128;
        let sticky = u128::from(!shifted.is_multiple_of(count as u128));
        let value = ((q << 1) | sticky) as f64 * 2f64.powi(-30 - shift as i32 - 1);
        if units < 0 {
            -value
        } else {
            value
        }
    }

    #[test]
    fn a_subnormal_mean_rounds_at_the_smallest_subnormal() {
        let mut s = ExactSum::zero();
        s.add(f64::from_bits(3), 1); // 3 * 2^-1074
        assert_eq!(
            s.div_round(2),
            f64::from_bits(2),
            "1.5 ulp rounds to even, 2"
        );
        assert_eq!(s.div_round(4), f64::from_bits(1), "0.75 ulp rounds to 1");
        assert_eq!(
            s.div_round(8).to_bits(),
            0.0f64.to_bits(),
            "0.375 ulp rounds to +0.0"
        );
    }

    #[test]
    fn bytes_round_trip() {
        let mut s = ExactSum::zero();
        s.add(-0.1, 7);
        let bytes = s.to_be_bytes();
        assert_eq!(bytes.len(), EXACT_BYTES);
        assert_eq!(ExactSum::from_be_bytes(&bytes), Some(s));
        assert_eq!(ExactSum::from_be_bytes(&bytes[1..]), None);
    }

    // The tests below check the whole f64 range against references that do
    // not share `round`'s or `div_round`'s code: single IEEE 754 operations
    // (each correctly rounded, ties to even), `i128 as f64`, and an exact
    // comparison with the midpoints between neighbouring f64 values.

    /// A random finite, non-zero `f64` of either sign over the whole range,
    /// drawing subnormals and the extreme exponents far more often than a
    /// uniform choice of bits would.
    fn wide_finite(rng: &mut SplitMix64) -> f64 {
        const LARGEST_FINITE_EXPONENT: u64 = 0x7fe;
        loop {
            let exponent = match rng.below(8) {
                0 => 0,
                1 => 1,
                2 => LARGEST_FINITE_EXPONENT,
                3 => LARGEST_FINITE_EXPONENT - 1,
                _ => rng.below(LARGEST_FINITE_EXPONENT + 1),
            };
            let x = f64::from_bits(
                (rng.below(2) << 63) | (exponent << 52) | (rng.next_u64() & ((1 << 52) - 1)),
            );
            if x != 0.0 {
                return x;
            }
        }
    }

    /// A random finite, non-zero `f64` whose exponent is at most 60 below `x`'s,
    /// so that `x + y` usually has to round.
    fn near(rng: &mut SplitMix64, x: f64) -> f64 {
        let exponent = ((x.to_bits() >> 52) & 0x7ff).saturating_sub(rng.below(60));
        let y = f64::from_bits(
            (rng.below(2) << 63) | (exponent << 52) | (rng.next_u64() & ((1 << 52) - 1)),
        );
        if y == 0.0 {
            f64::from_bits(1)
        } else {
            y
        }
    }

    fn of(x: f64, weight: i64) -> ExactSum {
        let mut s = ExactSum::zero();
        s.add(x, weight);
        s
    }

    #[test]
    fn one_weighted_term_rounds_like_ieee_multiplication() {
        let mut rng = SplitMix64(5);
        for _ in 0..20_000 {
            let x = wide_finite(&mut rng);
            // |w| < 2^20, so `w as f64` is exact and `x * w` rounds once.
            let w = match rng.below(3) {
                0 => 1,
                1 => -1,
                _ => rng.below(1 << 21) as i64 - (1 << 20),
            };
            let want = x * w as f64;
            let want = if want == 0.0 { 0.0 } else { want };
            assert_eq!(of(x, w).round().to_bits(), want.to_bits(), "{x:e} * {w}");
        }
    }

    #[test]
    fn two_terms_round_like_ieee_addition() {
        let mut rng = SplitMix64(6);
        for _ in 0..20_000 {
            let x = wide_finite(&mut rng);
            let y = if rng.below(4) == 0 {
                wide_finite(&mut rng)
            } else {
                near(&mut rng, x)
            };
            let mut s = of(x, 1);
            s.add(y, 1);
            // IEEE gives +0.0 for x + (-x) under round to nearest, as `round` must.
            assert_eq!(s.round().to_bits(), (x + y).to_bits(), "{x:e} + {y:e}");
        }
    }

    #[test]
    fn one_term_divided_rounds_like_ieee_division() {
        let mut rng = SplitMix64(7);
        for _ in 0..20_000 {
            let x = wide_finite(&mut rng);
            // d < 2^53, so `d as f64` is exact and `x / d` rounds once,
            // including to a subnormal or to a signed zero.
            let d = match rng.below(3) {
                0 => 1 << rng.below(53),
                1 => 1 + rng.below(16),
                _ => 1 + rng.below(1 << 53),
            } as i64;
            let want = x / d as f64;
            assert_eq!(
                of(x, 1).div_round(d).to_bits(),
                want.to_bits(),
                "{x:e} / {d}"
            );
        }
    }

    #[test]
    fn a_negative_mean_that_underflows_is_negative_zero() {
        // IEEE 754: -3 * 2^-1074 / 8 is -0.375 ulp, which rounds to -0.0.
        assert_eq!(
            of(f64::from_bits(3), -1).div_round(8).to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            of(f64::from_bits(3), -1).div_round(8).to_bits(),
            (-3e-324f64 / 8.0).to_bits()
        );
    }

    #[test]
    fn add_int_rounds_like_an_i128_cast() {
        let mut rng = SplitMix64(8);
        for n in [i128::MIN, i128::MIN + 1, i128::MAX, -1, 1] {
            let mut s = ExactSum::zero();
            s.add_int(n);
            assert_eq!(s.round().to_bits(), (n as f64).to_bits(), "{n}");
        }
        for _ in 0..20_000 {
            let bits = (u128::from(rng.next_u64()) << 64) | u128::from(rng.next_u64());
            let n = (bits >> rng.below(128)) as i128;
            let mut s = ExactSum::zero();
            s.add_int(n);
            assert_eq!(s.round().to_bits(), (n as f64).to_bits(), "{n}");
        }
        let mut s = ExactSum::zero();
        s.add_int(i128::MIN);
        s.add_int(i128::MAX);
        s.add_int(1);
        assert!(s.is_zero());
    }

    #[test]
    fn add_sum_equals_adding_every_term() {
        let mut rng = SplitMix64(9);
        for _ in 0..500 {
            let terms: Vec<(f64, i64)> = (0..1 + rng.below(20))
                .map(|_| (wide_finite(&mut rng), rng.below(2001) as i64 - 1000))
                .collect();
            let split = rng.below(terms.len() as u64 + 1) as usize;
            let mut all = ExactSum::zero();
            let (mut left, mut right) = (ExactSum::zero(), ExactSum::zero());
            for (i, &(x, w)) in terms.iter().enumerate() {
                all.add(x, w);
                if i < split {
                    left.add(x, w);
                } else {
                    right.add(x, w);
                }
            }
            left.add_sum(&right);
            assert_eq!(left, all, "{terms:?}");
        }
    }

    /// The gaps from `r` to the next `f64` below and above it, treating
    /// 2^1024 as the value after `f64::MAX` (where round to nearest switches
    /// to infinity).
    fn gaps(r: f64) -> (f64, f64) {
        let after_max = 2f64.powi(971);
        let up = if r.next_up().is_finite() {
            r.next_up() - r
        } else {
            after_max
        };
        let down = if r.next_down().is_finite() {
            r - r.next_down()
        } else {
            after_max
        };
        (down, up)
    }

    /// The sign of `sum`: -1, 0 or 1.
    fn signum(sum: &ExactSum) -> i32 {
        if sum.is_zero() {
            0
        } else if sum.is_negative() {
            -1
        } else {
            1
        }
    }

    /// Asserts that `r` is `sum / d` rounded to nearest, ties to even, by
    /// comparing `2 * sum` exactly with `d` times the doubled midpoints
    /// between `r` and its neighbours, and checks `r`'s sign.
    fn assert_nearest(sum: &ExactSum, d: i64, r: f64, context: &dyn std::fmt::Debug) {
        let mut twice = sum.clone();
        twice.add_sum(sum);
        match signum(sum) {
            0 => assert_eq!(r.to_bits(), 0.0f64.to_bits(), "exact zero, {context:?}"),
            1 => assert!(r.is_sign_positive(), "sign, {context:?}"),
            _ => assert!(r.is_sign_negative(), "sign, {context:?}"),
        }
        // `2 * sum - d * (2 * base + offset)`.
        let minus = |base: f64, offset: f64| {
            let mut t = twice.clone();
            t.add(base, -2 * d);
            t.add(offset, -d);
            signum(&t)
        };
        if r.is_infinite() {
            // At least the midpoint between ±MAX and ±2^1024 (a tie goes to
            // infinity, since MAX's significand is odd).
            let edge = minus(f64::MAX.copysign(r), 2f64.powi(971).copysign(r));
            assert!(
                edge * r.signum() as i32 >= 0,
                "beyond the range, {context:?}"
            );
            return;
        }
        let (down, up) = gaps(r);
        let even = r.to_bits() & 1 == 0;
        let above = minus(r, up);
        let below = minus(r, -down);
        assert!(
            above < 0 || (above == 0 && even),
            "above the upper midpoint, {context:?}"
        );
        assert!(
            below > 0 || (below == 0 && even),
            "below the lower midpoint, {context:?}"
        );
    }

    /// Random weighted terms over the whole range, or, half the time, an
    /// exact tie `d * (r + gap / 2)` with an optional tiny term that breaks it.
    fn wide_sum(rng: &mut SplitMix64, d: i64) -> ExactSum {
        let mut s = ExactSum::zero();
        if rng.below(2) == 0 {
            for _ in 0..1 + rng.below(12) {
                s.add(wide_finite(rng), rng.below(2001) as i64 - 1000);
            }
            return s;
        }
        let r = wide_finite(rng);
        let half_gap = gaps(r).1 / 2.0;
        if half_gap == 0.0 || !r.next_up().is_finite() {
            return of(r, d);
        }
        s.add(r, d);
        s.add(half_gap, d);
        match rng.below(3) {
            0 => s.add(f64::from_bits(1), 1),
            1 => s.add(f64::from_bits(1), -1),
            _ => {}
        }
        s
    }

    #[test]
    fn round_is_nearest_over_the_whole_range() {
        let mut rng = SplitMix64(10);
        for _ in 0..5_000 {
            let s = wide_sum(&mut rng, 1);
            assert_nearest(&s, 1, s.round(), &s);
        }
    }

    #[test]
    fn div_round_is_nearest_over_the_whole_range() {
        let mut rng = SplitMix64(11);
        for _ in 0..5_000 {
            let d = match rng.below(3) {
                0 => 1 + rng.below(8),
                1 => 1 + rng.below(1 << 32),
                _ => 1 + rng.below(1 << 62),
            } as i64;
            let s = wide_sum(&mut rng, d);
            assert_nearest(&s, d, s.div_round(d), &(&s, d));
        }
    }

    #[test]
    fn div_round_by_a_huge_count_matches_an_i128_reference() {
        let mut rng = SplitMix64(12);
        for _ in 0..2000 {
            let terms: Vec<_> = (0..1 + rng.below(30)).map(|_| term(&mut rng)).collect();
            let units: i128 = terms
                .iter()
                .map(|&(m, k, w)| (i128::from(m) << (k + 30)) * i128::from(w))
                .sum();
            if units == 0 {
                continue;
            }
            let count = 1 + rng.below(i64::MAX as u64) as i64;
            assert_eq!(
                sum_of(&terms).div_round(count).to_bits(),
                mean_reference(units, count).to_bits(),
                "{terms:?} / {count}"
            );
        }
    }

    #[test]
    fn the_stored_bytes_are_big_endian_twos_complement() {
        // One is 2^1074 units: bit 50 of limb 16, the 19th limb from the top.
        let mut one = ExactSum::zero();
        one.add_int(1);
        let bytes = one.to_be_bytes();
        let limb_16 = (EXACT_LIMBS - 1 - 16) * 8;
        let mut want = [0u8; EXACT_BYTES];
        want[limb_16..limb_16 + 8].copy_from_slice(&(1u64 << 50).to_be_bytes());
        assert_eq!(bytes, want);
        // -2^-1074 is every bit set.
        assert_eq!(of(f64::from_bits(1), -1).to_be_bytes(), [0xff; EXACT_BYTES]);
    }

    #[test]
    fn a_stored_sum_outside_the_reachable_range_is_refused() {
        // Every reachable sum is below 2^2225 units in magnitude, so bits
        // 2225 to 2239 copy the sign bit. 2^2224 is accepted; 2^2225 is not.
        let with_bit = |bit: usize| {
            let mut bytes = [0u8; EXACT_BYTES];
            bytes[EXACT_BYTES - 1 - bit / 8] = 1 << (bit % 8);
            bytes
        };
        assert!(ExactSum::from_be_bytes(&with_bit(2224)).is_some());
        assert_eq!(ExactSum::from_be_bytes(&with_bit(2225)), None);
        assert_eq!(ExactSum::from_be_bytes(&with_bit(2239)), None);
        let mut negative = [0xff; EXACT_BYTES];
        assert!(ExactSum::from_be_bytes(&negative).is_some());
        negative[0] = 0x7f;
        assert_eq!(ExactSum::from_be_bytes(&negative), None);
        // The largest single term, f64::MAX * i64::MIN, round-trips.
        let big = of(f64::MAX, i64::MIN);
        assert_eq!(ExactSum::from_be_bytes(&big.to_be_bytes()), Some(big));
    }
}
