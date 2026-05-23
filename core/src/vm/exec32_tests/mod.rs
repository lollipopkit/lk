use super::*;
use std::sync::Arc;

use crate::{
    val::{CallableValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal},
    vm::{
        ConstHeapValue32, ConstPool32, Instr32, NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32,
        Opcode32, RuntimeCallable32, VmContext,
    },
};

mod basic;
mod calls;
mod container;
mod gc_cell_error;
mod native;
