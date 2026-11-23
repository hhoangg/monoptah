use super::*;

#[test]
fn version_parsing_orders_numbers_not_text() {
    assert_eq!(parse_version("v22.1.0"), Some((22, 1, 0)));
    assert_eq!(parse_version("22.1.0"), Some((22, 1, 0)));
    assert_eq!(parse_version("lts"), None);
    assert_eq!(parse_version("v1.2"), None);
    assert_eq!(parse_version("v1.2.3.4"), None);
}
