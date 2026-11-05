use chisel::cli;

fn main() {
    let code = match cli::entry() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    };
    std::process::exit(code);
}
