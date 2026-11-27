#[path = "../memory.rs"]
mod memory;
#[path = "../platform.rs"]
mod platform;
#[path = "../primitives.rs"]
mod primitives;

fn main() {
    memory::run(&mut std::env::args().skip(1));
}
