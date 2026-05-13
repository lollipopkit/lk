use lk_core::val::Val;

fn main() {
    println!("size_of Val: {}", std::mem::size_of::<Val>());
    println!("align_of Val: {}", std::mem::align_of::<Val>());
    println!("size_of Option<Val>: {}", std::mem::size_of::<Option<Val>>());
    println!("size_of Vec<Val>: {}", std::mem::size_of::<Vec<Val>>());
}
