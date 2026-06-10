pub mod socket;
pub mod tcp;
pub mod udp;

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
#[stdlib_module(name = "net", docs = "Network socket helpers")]
pub struct NetModule;

#[lk_stdlib_common::stdlib_exports(
    children(
        socket = socket::NetSocketModule,
        tcp = tcp::NetTcpModule,
        udp = udp::NetUdpModule,
    )
)]
impl NetModule {}
