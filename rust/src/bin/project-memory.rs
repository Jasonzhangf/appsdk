#[path = "../memory.rs"]
mod memory;

fn main() {
    memory::run(&mut std::env::args().skip(1));
}
