#[test]
fn one_line() {
    // the body of web_search.rs:64
    let h = " a\u{85}b \u{feff} c\u{feff}";
    println!("{:?}", h.split_whitespace().collect::<Vec<_>>().join(" "));
}
