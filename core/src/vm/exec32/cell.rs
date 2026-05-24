use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal};

use super::Executor32;

impl Executor32 {
    pub(super) fn load_cell_value(&self, cell_register: u8) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read(cell_register)? else {
            bail!("LoadCellVal expected UpvalCell object");
        };
        match self
            .state
            .heap
            .get(*handle)
            .ok_or_else(|| anyhow!("LoadCellVal heap object {} out of bounds", handle.index()))?
        {
            HeapValue::UpvalCell(value) => Ok(value.clone()),
            other => bail!("LoadCellVal expected UpvalCell, got {}", other.type_name()),
        }
    }

    pub(super) fn store_cell_value(&mut self, cell_register: u8, src_register: u8, move_value: bool) -> Result<()> {
        let value = if move_value {
            self.take(src_register)?
        } else {
            self.read(src_register)?.clone()
        };
        let RuntimeVal::Obj(handle) = self.read(cell_register)?.clone() else {
            bail!("StoreCellVal expected UpvalCell object");
        };
        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("StoreCellVal heap object {} out of bounds", handle.index()))?
        {
            HeapValue::UpvalCell(slot) => {
                *slot = value;
                Ok(())
            }
            other => bail!("StoreCellVal expected UpvalCell, got {}", other.type_name()),
        }
    }
}
