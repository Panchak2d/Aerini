//! Argument parser utilities: paren matching and comma splitting.

/// Returns the string inside the outermost `()`.
/// `input` begins immediately after the opening `(`.
pub(super) fn find_matching_close(input: &str) -> Option<&str> {
    let mut depth = 1i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;

    for (i, c) in input.char_indices() {
        if escape_next { escape_next = false; continue; }
        if c == '\\' && (in_single || in_double) { escape_next = true; continue; }
        if c == '\'' && !in_double { in_single = !in_single; continue; }
        if c == '"'  && !in_single { in_double = !in_double; continue; }
        if !in_single && !in_double {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 { return Some(&input[..i]); }
                }
                _ => {}
            }
        }
    }
    None
}

/// Split `args` by commas, respecting nested parens and string literals.
pub(super) fn split_args(args: &str) -> Vec<&str> {
    if args.trim().is_empty() { return vec![]; }
    let mut result = Vec::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;
    let mut start = 0;

    for (i, c) in args.char_indices() {
        if escape_next { escape_next = false; continue; }
        if c == '\\' && (in_single || in_double) { escape_next = true; continue; }
        if c == '\'' && !in_double { in_single = !in_single; continue; }
        if c == '"'  && !in_single { in_double = !in_double; continue; }
        if !in_single && !in_double {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => { result.push(&args[start..i]); start = i + 1; }
                _ => {}
            }
        }
    }
    result.push(&args[start..]);
    result
}
