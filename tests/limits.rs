// Untrusted nesting is bounded: a hostile source must be a parse error, not a
// stack overflow.

#[test]
fn deep_expression_nesting_is_a_parse_error() {
    let src = format!(
        "action main():\n    let x = {}1{}\n",
        "(".repeat(400),
        ")".repeat(400)
    );
    let err = kaze::check_source(&src, "<deep-expr>").expect_err("nesting is bounded");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("nests more than"), "{}", err.message);
}

#[test]
fn deep_unary_and_not_nesting_is_a_parse_error() {
    let unary = format!("action main():\n    let x = {}1\n", "-".repeat(400));
    let err = kaze::check_source(&unary, "<deep-unary>").expect_err("nesting is bounded");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("nests more than"), "{}", err.message);

    let not = format!("action main():\n    let x = {}1 == 1\n", "not ".repeat(400));
    let err = kaze::check_source(&not, "<deep-not>").expect_err("nesting is bounded");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("nests more than"), "{}", err.message);
}

#[test]
fn deep_block_nesting_is_a_parse_error() {
    let mut src = String::from("needs: log\naction main():\n");
    for depth in 0..400 {
        src.push_str(&"    ".repeat(depth + 1));
        src.push_str("if 1 == 1:\n");
    }
    src.push_str(&"    ".repeat(401));
    src.push_str("log \"x\"\n");
    let err = kaze::check_source(&src, "<deep-block>").expect_err("nesting is bounded");
    assert_eq!(err.code, "E_KAZE_PARSE");
    assert!(err.message.contains("nests more than"), "{}", err.message);
}

#[test]
fn the_collection_heap_is_bounded() {
    // Fuel alone would let this loop allocate until the process dies; the
    // heap budget stops it first with a typed, non-catchable error.
    let src = "action main():\n    let filler = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]\n    while 1 == 1:\n        let x = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]\n";
    let opts = kaze::RunOptions { fuel: 200_000, ..Default::default() };
    let err = kaze::run_source(src, "<heap>", &opts).expect_err("the heap is bounded");
    assert_eq!(err.code, "E_KAZE_MEMORY");
}

#[test]
fn ordinary_nesting_still_parses() {
    let mut src = String::from("needs: log\naction main():\n");
    for depth in 0..20 {
        src.push_str(&"    ".repeat(depth + 1));
        src.push_str("if 1 == 1:\n");
    }
    src.push_str(&"    ".repeat(21));
    src.push_str("log \"x\"\n");
    kaze::check_source(&src, "<normal>").expect("twenty levels is ordinary");
}
