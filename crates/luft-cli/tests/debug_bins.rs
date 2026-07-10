#[test]
fn print_bin_env_vars() {
    for (k, v) in std::env::vars() {
        if k.starts_with("CARGO") {
            eprintln!("{k}={v}");
        }
    }
}
