use crate::val::Type;

use super::analysis::{PerfKeyFact, PerfValueKind, PerformanceFacts};

impl PerformanceFacts {
    pub fn value_kind(&self, reg: u16) -> PerfValueKind {
        self.register(reg)
            .map(|fact| fact.value.kind)
            .unwrap_or(PerfValueKind::Unknown)
    }

    pub fn value_type(&self, reg: u16) -> Option<Type> {
        type_from_value_kind(self.value_kind(reg))
    }

    pub fn list_value_kind(&self, reg: u16) -> Option<PerfValueKind> {
        self.register(reg)
            .and_then(|fact| fact.list.map(|list| list.value_kind))
    }

    pub fn list_value_type(&self, reg: u16) -> Option<Type> {
        self.list_value_kind(reg).and_then(type_from_value_kind)
    }

    pub fn list_known_len(&self, reg: u16) -> Option<usize> {
        self.register(reg)
            .and_then(|fact| fact.list.and_then(|list| list.known_len))
    }

    pub fn map_value_kind(&self, reg: u16) -> Option<PerfValueKind> {
        self.register(reg).and_then(|fact| fact.map.map(|map| map.value_kind))
    }

    pub fn map_value_type(&self, reg: u16) -> Option<Type> {
        self.map_value_kind(reg).and_then(type_from_value_kind)
    }

    pub fn known_key(&self, pc: usize) -> Option<&PerfKeyFact> {
        self.key_ops.get(pc).and_then(Option::as_ref)
    }

    pub fn dead_write(&self, pc: usize) -> bool {
        self.is_dead_write(pc)
    }
}

fn type_from_value_kind(kind: PerfValueKind) -> Option<Type> {
    match kind {
        PerfValueKind::Nil => Some(Type::Nil),
        PerfValueKind::Bool => Some(Type::Bool),
        PerfValueKind::Int => Some(Type::Int),
        PerfValueKind::Float => Some(Type::Float),
        PerfValueKind::String => Some(Type::String),
        PerfValueKind::List => Some(Type::List(Box::new(Type::Any))),
        PerfValueKind::Map => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
        PerfValueKind::Object | PerfValueKind::Unknown => None,
    }
}
