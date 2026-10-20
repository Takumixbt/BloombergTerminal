//! Parse House PTR PDFs and print the extracted trades: `cargo run --example ptr -- file.pdf`
fn main() {
    for p in std::env::args().skip(1) {
        match hd_congress::house::parse_ptr_pdf(&std::fs::read(&p).unwrap()) {
            Ok(r) => {
                for t in r.trades {
                    println!(
                        "{:?} | {:?} | {:?} | {} | {} | {:?}",
                        t.owner, t.ticker, t.tx_type, t.tx_date, t.asset_name, t.amount
                    );
                }
            }
            Err(e) => println!("{p}: {e}"),
        }
    }
}
