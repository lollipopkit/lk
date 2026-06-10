pub mod file;
pub mod std_io;

pub mod bytes {
    pub use lk_stdlib_bytes::*;
}
pub mod resource {
    pub use lk_stdlib_common::resource::*;
}
pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "io", docs = "Input, output, and file resources")]
pub struct IoModule;

#[lk_stdlib_common::stdlib_exports(children(std = std_io::IoStdModule, file = file::IoFileModule))]
impl IoModule {}
