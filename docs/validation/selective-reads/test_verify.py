"""Python log fixtures only; never invoke Cargo or native test binaries."""
import unittest

from verify import parse_log


def header(name):
    return f"     Running unittests {name}.rs (target/debug/deps/{name}-0123abcd)\n"


def summary(passed=1):
    return (f"test result: ok. {passed} passed; 0 failed; 0 ignored; "
            "0 measured; 0 filtered out; finished in 0.0s\n")


class BinaryCompleteness(unittest.TestCase):
    def test_rejects_trailing_started_binary_without_failures(self):
        for trailing in (header("unfinished"), "   Doc-tests unfinished\n"):
            with self.subTest(trailing=trailing), self.assertRaisesRegex(ValueError, "partial binary"):
                parse_log(header("complete") + summary() + trailing + "running 0 tests\n")

    def test_rejects_missing_middle_summary(self):
        for following in (header("next"), "   Doc-tests next\n"):
            with self.subTest(following=following), self.assertRaisesRegex(ValueError, "Missing binary summary"):
                parse_log(header("complete") + summary() + header("middle") + following + summary())

    def test_rejects_double_summary(self):
        with self.assertRaisesRegex(ValueError, "Summary without binary"):
            parse_log(header("fixture") + summary() + summary())

    def test_accepts_multiple_complete_binaries_and_doctests(self):
        records, red = parse_log(header("first") + summary(2) + header("second") + summary(3)
                                 + "   Doc-tests fixture\n" + summary(0))
        self.assertEqual([record["binary"] for record in records], [
            "unittests first.rs [first]", "unittests second.rs [second]", "Doc-tests fixture"])
        self.assertEqual([record["passed"] for record in records], [2, 3, 0])
        self.assertEqual(red, [])

    def test_accepts_semantic_red_cargo_footers(self):
        text = (header("fixture") + "test guard ... FAILED\n"
                "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.0s\n"
                "error: test failed, to rerun pass `--lib`\nerror: 1 target failed:\n")
        records, red = parse_log(text)
        self.assertEqual(records[0]["failed"], 1)
        self.assertEqual(red, ["guard"])
        with self.assertRaisesRegex(ValueError, "Compilation invalid"):
            parse_log(text + "error[E0001]: fixture\n")


if __name__ == "__main__":
    unittest.main()
