use anyhow::{Result, anyhow, bail};

use crate::val::RuntimeVal;

use super::Executor;

impl Executor {
    pub(super) fn read_global(&self, slot: u16) -> Result<RuntimeVal> {
        self.state
            .globals
            .get(slot as usize)
            .cloned()
            .ok_or_else(|| anyhow!("global slot {} out of bounds", slot))
    }

    pub(super) fn write_global(&mut self, slot: u16, value: RuntimeVal) -> Result<()> {
        let Some(target) = self.state.globals.get_mut(slot as usize) else {
            bail!("global slot {} out of bounds", slot);
        };
        *target = value;
        Ok(())
    }
}
