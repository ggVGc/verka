use super::*;

#[test]
fn csv_ignores_empty_values_and_whitespace() {
    assert_eq!(csv(" a, ,b "), vec!["a", "b"]);
}

#[test]
fn hex_dump_lays_out_sixteen_bytes_per_line() {
    let dump = hex_dump(b"hi\0there, attachment!");
    let mut lines = dump.lines().skip(2);
    assert_eq!(
        lines.next().unwrap(),
        "00000000  68 69 00 74 68 65 72 65 2c 20 61 74 74 61 63 68  hi.there, attach"
    );
    assert_eq!(
        lines.next().unwrap(),
        "00000010  6d 65 6e 74 21                                   ment!"
    );
}

#[test]
fn hex_dump_reports_truncated_bytes() {
    let dump = hex_dump(&vec![0u8; 8 * 1024 + 3]);
    assert!(dump.contains("Binary payload · 8195 bytes"));
    assert!(dump.contains("… 3 more byte(s)"));
}

#[test]
fn parsers_accept_documented_values() {
    assert_eq!(author("machine").unwrap(), Author::Machine);
    assert_eq!(dep_kind("depends-on").unwrap(), DepKind::DependsOn);
    assert_eq!(
        verification_outcome("abandoned").unwrap(),
        VerificationOutcome::Abandoned
    );
}
