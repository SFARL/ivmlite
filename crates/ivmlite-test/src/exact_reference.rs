//! M2c spec §6: the independent reference for REAL results. `fsum` is
//! Shewchuk's exact summation, the algorithm behind Python's `math.fsum`;
//! `exact_mean` divides the exact sum by a count and rounds once, with no
//! error. Neither shares code with `ivmlite_core::ExactSum`, the engine's
//! superaccumulator they judge.

use std::cmp::Ordering;

/// ε in KBN's error bound: the unit roundoff, 2^-53 (M2c spec §6, amendment 7).
pub const UNIT_ROUNDOFF: f64 = f64::EPSILON / 2.0;

/// The exact sum of `terms`, rounded once to the nearest `f64`, ties to even
/// (CPython's `msum` with its half-way correction). An exact zero is +0.0.
///
/// # Panics
/// If a partial sum leaves the finite range: the terms must be finite, and
/// their partial sums too (CPython raises "intermediate overflow" there).
pub fn fsum(terms: &[f64]) -> f64 {
    // Non-overlapping partials, in increasing magnitude, whose exact sum is
    // the exact sum of the terms seen so far.
    let mut partials: Vec<f64> = Vec::new();
    for &term in terms {
        let mut x = term;
        let mut kept = 0;
        for i in 0..partials.len() {
            let mut y = partials[i];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[kept] = lo;
                kept += 1;
            }
            x = hi;
        }
        partials.truncate(kept);
        if x != 0.0 {
            partials.push(x);
        }
    }
    let Some(mut n) = partials.len().checked_sub(1) else {
        return 0.0;
    };
    // Add the partials from the top, stopping at the first inexact sum.
    let mut hi = partials[n];
    let mut lo = 0.0;
    while n > 0 {
        let x = hi;
        n -= 1;
        let y = partials[n];
        hi = x + y;
        let yr = hi - x;
        lo = y - yr;
        if lo != 0.0 {
            break;
        }
    }
    // Half-way correction: `hi + lo` sat exactly half-way and was rounded to
    // even, but the partials below push the exact sum past the half.
    if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
        let y = lo * 2.0;
        let x = hi + y;
        let yr = x - hi;
        if y == yr {
            hi = x;
        }
    }
    assert!(
        hi.is_finite(),
        "fsum's partial sums left the finite range: {terms:?}"
    );
    hi
}

/// Three `f64`s whose exact sum is `n`: its magnitude in 43-bit pieces, each
/// exactly representable, with `n`'s sign.
pub fn int_pieces(n: i128) -> [f64; 3] {
    const PIECE_BITS: u32 = 43;
    let m = n.unsigned_abs();
    let mask = (1u128 << PIECE_BITS) - 1;
    let piece = |shift: u32| (((m >> shift) & mask) as f64) * 2f64.powi(shift as i32);
    let pieces = [piece(2 * PIECE_BITS), piece(PIECE_BITS), piece(0)];
    if n < 0 {
        pieces.map(|p| -p)
    } else {
        pieces
    }
}

/// `a * b` as an exact pair `(p, e)` with `p + e = a·b`: the FMA computes the
/// rounding error of the product exactly.
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    (p, a.mul_add(b, -p))
}

/// The sign of `2·Σterms − count·a − count·b`, which compares the exact mean
/// with the midpoint of `a` and `b`. Exact: doubling is exact, `two_product`
/// is exact for an integral `count` (its error is a multiple of `ulp(b)`, at
/// most 2^53 of them), and `fsum` rounds an exact sum's sign correctly.
fn twice_sum_minus(terms: &[f64], count: f64, a: f64, b: f64) -> Ordering {
    let (pa, ea) = two_product(count, a);
    let (pb, eb) = two_product(count, b);
    let mut all: Vec<f64> = terms.iter().map(|t| 2.0 * t).collect();
    all.extend([-pa, -ea, -pb, -eb]);
    fsum(&all)
        .partial_cmp(&0.0)
        .expect("fsum never returns NaN")
}

/// The exact `Σterms / count`, rounded once to the nearest `f64`, ties to
/// even. A negative mean that rounds to zero is -0.0, as IEEE 754 division
/// gives; an exact zero is +0.0.
///
/// It starts from the rounded sum's quotient, then moves to a neighbour while
/// the exact mean lies past their midpoint, judged exactly by
/// `twice_sum_minus`.
///
/// # Panics
/// Unless `0 < count < 2^53`. Each term's double must stay finite, as must
/// `fsum`'s partials.
pub fn exact_mean(terms: &[f64], count: i64) -> f64 {
    assert!(
        count > 0 && count < 1 << 53,
        "a mean of a positive count below 2^53, got {count}"
    );
    let sum = fsum(terms);
    if sum == 0.0 {
        return 0.0;
    }
    let n = count as f64;
    let even = |a: f64, b: f64| if a.to_bits() & 1 == 0 { a } else { b };
    let mut q = sum / n;
    loop {
        let up = q.next_up();
        match twice_sum_minus(terms, n, q, up) {
            Ordering::Greater => {
                q = up;
                continue;
            }
            Ordering::Equal => return even(q, up),
            Ordering::Less => {}
        }
        let down = q.next_down();
        match twice_sum_minus(terms, n, q, down) {
            Ordering::Less => {
                q = down;
                continue;
            }
            Ordering::Equal => return even(q, down),
            Ordering::Greater => return q,
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    use super::*;

    /// 2^e exactly, subnormals included (`powi` is not exact far from 1).
    fn pow2(e: i32) -> f64 {
        if e >= -1022 {
            f64::from_bits(u64::try_from(e + 1023).unwrap() << 52)
        } else {
            f64::from_bits(1u64 << (e + 1074))
        }
    }

    #[test]
    fn fsum_rounds_half_way_cases_correctly() {
        // CPython's test_math.test_fsum vectors (checked with math.fsum).
        let p53 = pow2(53);
        assert_eq!(fsum(&[p53, -0.5, -pow2(-54)]), p53 - 1.0);
        assert_eq!(fsum(&[p53, 1.0, pow2(-100)]), p53 + 2.0);
        assert_eq!(fsum(&[p53 + 10.0, 1.0, pow2(-100)]), p53 + 12.0);
        assert_eq!(fsum(&[p53 - 4.0, 0.5, pow2(-54)]), p53 - 3.0);
        assert_eq!(fsum(&[1e16, 1.0, 1e-16]), 10_000_000_000_000_002.0);
        assert_eq!(fsum(&[1e-16, 1.0, 1e16]), 10_000_000_000_000_002.0);
    }

    #[test]
    fn fsum_is_exact_where_naive_summation_is_not() {
        assert_eq!(fsum(&[1e16, 1.0, -1e16, 1.0]), 2.0);
        assert_eq!(fsum(&[0.1, 0.2, 0.3]), 0.6);
        assert_eq!(fsum(&[0.1; 10]), 1.0);
        assert_eq!(fsum(&[]), 0.0);
        // More of CPython's test_fsum vectors.
        assert_eq!(
            fsum(&[1e100, 1.0, -1e100, 1e-100, 1e50, -1.0, -1e50]),
            1e-100
        );
        let near_one = 1.0 - pow2(-53);
        assert_eq!(fsum(&[1e16 - 2.0, near_one, -(1e16 - 2.0), -near_one]), 0.0);
        // Telescoping: each difference is exact (Sterbenz), so the exact sum is -1.
        let mut telescoping: Vec<f64> = (0..1000)
            .map(|i| 1.7f64.powf(f64::from(i + 1)) - 1.7f64.powf(f64::from(i)))
            .collect();
        telescoping.push(-1.7f64.powf(1000.0));
        assert_eq!(fsum(&telescoping), -1.0);
        // Many partials, spread over the whole exponent range.
        let mut spread: Vec<f64> = (-1074..972)
            .step_by(2)
            .map(|n| pow2(n) - pow2(n + 50) + pow2(n + 52))
            .collect();
        spread.push(-pow2(1022));
        // 0x1.5555555555555p+970
        assert_eq!(fsum(&spread), f64::from_bits(0x7C95_5555_5555_5555));
    }

    #[test]
    fn fsum_of_zeros_is_positive_zero() {
        // M2c spec §4: the sum is the exact value rounded once; an exact zero
        // is +0.0, as IEEE 754 gives for x + (-x).
        assert_eq!(fsum(&[-0.0]).to_bits(), 0.0f64.to_bits());
        assert_eq!(fsum(&[-0.0, -0.0]).to_bits(), 0.0f64.to_bits());
        assert_eq!(fsum(&[2.5, -2.5]).to_bits(), 0.0f64.to_bits());
    }

    /// Dyadic rationals k·2^-30 with |k| < 2^45: their exact sum is an `i128`
    /// times 2^-30, and an integer converts to the nearest `f64`, ties to even.
    #[test]
    fn fsum_matches_exact_dyadic_sums() {
        let mut rng = StdRng::seed_from_u64(7);
        let scale = pow2(-30);
        for _ in 0..500 {
            let len = rng.random_range(1..60);
            let ks: Vec<i64> = (0..len)
                .map(|_| rng.random_range(-(1i64 << 45)..(1i64 << 45)))
                .collect();
            let terms: Vec<f64> = ks.iter().map(|&k| k as f64 * scale).collect();
            let exact: i128 = ks.iter().map(|&k| i128::from(k)).sum();
            assert_eq!(fsum(&terms), exact as f64 * scale, "{ks:?}");
        }
    }

    #[test]
    fn exact_mean_is_correctly_rounded() {
        // fsum gives 0.6, and 0.6 / 3.0 is 0.19999999999999998: rounding
        // the sum before dividing is off by one ulp here.
        assert_eq!(exact_mean(&[0.1, 0.2, 0.3], 3), 0.2);
        assert_eq!(exact_mean(&[1.0, 2.0], 2), 1.5);
        assert_eq!(exact_mean(&[1.0], 3), 1.0 / 3.0);
        let p54 = pow2(54);
        // Exactly half-way: to even.
        assert_eq!(exact_mean(&[p54, 2.0], 2), pow2(53));
        assert_eq!(exact_mean(&[p54, 6.0], 2), pow2(53) + 4.0);
        assert_eq!(exact_mean(&[-0.1, -0.2, -0.3], 3), -0.2);
    }

    #[test]
    fn exact_mean_of_an_underflowing_negative_mean_is_negative_zero() {
        // IEEE 754 division, as SQLite's sum/count: a negative quotient that
        // rounds to zero keeps its sign.
        let tiny = pow2(-1074);
        assert_eq!(exact_mean(&[-tiny], 3).to_bits(), (-0.0f64).to_bits());
        assert_eq!(exact_mean(&[tiny], 3).to_bits(), 0.0f64.to_bits());
        assert_eq!(exact_mean(&[-0.0], 1).to_bits(), 0.0f64.to_bits());
        // Subnormal means are still exact: 3·2^-1074 / 2 is half-way, to even.
        assert_eq!(exact_mean(&[3.0 * tiny], 2), 2.0 * tiny);
    }

    /// `r` is the correctly rounded value of `num / den` (`num` and `den`
    /// scaled integers, `den > 0`): `|num − den·r|` is at most half of
    /// `den·ulp(r)`, and at exactly half `r` is even. Checked in `i128`, so
    /// `r`'s exponent must stay small.
    fn is_rounded_quotient(num: i128, den: i128, r: f64) -> bool {
        let (mantissa, exp) = decompose(r);
        // r = mantissa · 2^exp, ulp(r) = 2^exp (r normal, mantissa ≥ 2^52).
        let shift = u32::try_from(-exp).unwrap();
        let num = num << shift; // num / den, scaled by 2^-exp
        let err = num - den * mantissa;
        let twice = 2 * err.abs();
        twice < den || (twice == den && mantissa % 2 == 0)
    }

    fn decompose(r: f64) -> (i128, i32) {
        let bits = r.to_bits();
        let exp = i32::try_from((bits >> 52) & 0x7ff).unwrap() - 1075;
        let m = i128::from((bits & ((1 << 52) - 1)) | (1 << 52));
        (if r < 0.0 { -m } else { m }, exp)
    }

    #[test]
    fn exact_mean_matches_exact_dyadic_quotients() {
        // Terms k·2^-30 with |k| < 2^45: the mean is (Σk / n)·2^-30, so r is
        // correct when r·2^30 is the rounded quotient Σk / n.
        let mut rng = StdRng::seed_from_u64(11);
        for _ in 0..500 {
            let len: i32 = rng.random_range(1..40);
            let ks: Vec<i64> = (0..len)
                .map(|_| rng.random_range(-(1i64 << 45)..(1i64 << 45)))
                .collect();
            let terms: Vec<f64> = ks.iter().map(|&k| k as f64 * pow2(-30)).collect();
            let count = rng.random_range(1..=i64::from(len) + 3);
            let sum: i128 = ks.iter().map(|&k| i128::from(k)).sum();
            let r = exact_mean(&terms, count);
            if sum == 0 {
                assert_eq!(r.to_bits(), 0.0f64.to_bits());
                continue;
            }
            assert!(
                is_rounded_quotient(sum, i128::from(count), r * pow2(30)),
                "{ks:?} / {count} gave {r:?}"
            );
        }
    }

    #[test]
    fn int_pieces_sum_exactly() {
        for n in [
            0i128,
            1,
            -1,
            i128::from(i64::MAX) * 3,
            -(1i128 << 126) + 7,
            i128::MAX,
            i128::MIN,
        ] {
            let pieces = int_pieces(n);
            let sum: i128 = pieces.iter().map(|&p| p as i128).sum();
            assert_eq!(sum, n, "{pieces:?}");
        }
        // fsum of the pieces is the integer rounded once.
        let n = i128::from(i64::MAX) * 5 + 3;
        assert_eq!(fsum(&int_pieces(n)), n as f64);
    }
}
