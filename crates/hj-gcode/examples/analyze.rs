//! Print the analysis of a G-code file as JSON: `cargo run -p hj-gcode --example analyze -- FILE`.

fn main() {
    let path = std::env::args().nth(1).expect("usage: analyze FILE");
    let text = std::fs::read_to_string(&path).expect("readable file");
    let analysis = hj_gcode::analyze(&text);
    println!("{}", serde_json::to_string(&analysis).unwrap());
}
