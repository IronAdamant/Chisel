fn main() {
    let code = match chisel::cli::entry() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    };
    std::process::exit(code);
}
