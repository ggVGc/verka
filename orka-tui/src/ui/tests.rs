use super::*;

#[test]
fn live_tail_accounts_for_wrapped_output() {
    assert_eq!(wrapped_line_count("123456\nx", 3), 3);
    assert_eq!(wrapped_line_count("", 80), 1);
}
