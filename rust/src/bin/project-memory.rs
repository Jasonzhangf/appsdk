#[path = "../memory.rs"]
mod memory;
#[path = "../platform.rs"]
mod platform;

fn main() {
    memory::run(&mut std::env::args().skip(1));
}
