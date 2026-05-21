use std::sync::Arc;

use crate::vm::RegionPlan;
use crate::vm::bc32;
use crate::vm::bc32::Bc32Decoded;
use crate::vm::bytecode::Function;
use crate::vm::vm::Vm;
use crate::vm::vm::quickening::QuickeningSite;

use super::{AccessIc, CallIc, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, PackedHotEntry};

#[derive(Clone)]
pub(in crate::vm::vm) struct FunctionRuntimePlan {
    pub(in crate::vm::vm) func_key: usize,
    pub(in crate::vm::vm) reg_count: usize,
    pub(in crate::vm::vm) dispatch_sites: RuntimeDispatchSites,
    pub(in crate::vm::vm) region_plan: Option<Arc<RegionPlan>>,
}

impl FunctionRuntimePlan {
    #[inline(always)]
    pub(in crate::vm::vm) fn from_function(fun: &Function, region_plan: Option<Arc<RegionPlan>>) -> Self {
        let dispatch_sites = RuntimeDispatchSites::from_function(fun);
        Self::from_parts(fun, dispatch_sites, region_plan)
    }

    #[inline(always)]
    fn from_parts(fun: &Function, dispatch_sites: RuntimeDispatchSites, region_plan: Option<Arc<RegionPlan>>) -> Self {
        Self {
            func_key: fun as *const Function as usize,
            reg_count: fun.n_regs as usize,
            dispatch_sites,
            region_plan,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::vm::vm) struct RuntimeDispatchSites {
    opcode_len: usize,
    packed_len: Option<usize>,
    packed_site_len: usize,
    mixed_site_len: usize,
    packed_enabled: bool,
}

#[derive(Clone, Copy)]
pub(in crate::vm::vm) struct PackedDispatchCode<'a> {
    pub(in crate::vm::vm) words: &'a [u32],
    pub(in crate::vm::vm) decoded: Option<&'a Bc32Decoded>,
}

#[derive(Clone, Copy)]
pub(in crate::vm::vm) enum RuntimeDispatchMode<'a> {
    Packed(PackedDispatchCode<'a>),
    Opcode,
}

#[derive(Clone, Copy)]
pub(in crate::vm::vm) struct FrameDispatchPlan<'a> {
    fun: &'a Function,
    sites: RuntimeDispatchSites,
    mode: RuntimeDispatchMode<'a>,
}

impl<'a> FrameDispatchPlan<'a> {
    #[inline(always)]
    pub(in crate::vm::vm) const fn function(self) -> &'a Function {
        self.fun
    }

    #[inline(always)]
    pub(in crate::vm::vm) const fn mode(self) -> RuntimeDispatchMode<'a> {
        self.mode
    }
}

impl RuntimeDispatchSites {
    #[inline(always)]
    pub(in crate::vm::vm) const fn new(opcode_len: usize, packed_len: Option<usize>) -> Self {
        let packed_site_len = match packed_len {
            Some(len) => len,
            None => opcode_len,
        };
        let mixed_site_len = if opcode_len > packed_site_len {
            opcode_len
        } else {
            packed_site_len
        };
        Self {
            opcode_len,
            packed_len,
            packed_site_len,
            mixed_site_len,
            packed_enabled: packed_len.is_some(),
        }
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn from_function(fun: &Function) -> Self {
        let mut sites = Self::new(fun.code.len(), fun.code32.as_ref().map(Vec::len));
        sites.packed_enabled = function_supports_packed_dispatch(fun);
        sites
    }

    #[inline(always)]
    pub(in crate::vm::vm) const fn opcode_len(self) -> usize {
        self.opcode_len
    }

    #[inline(always)]
    #[cfg(test)]
    pub(in crate::vm::vm) const fn packed_len(self) -> Option<usize> {
        self.packed_len
    }

    #[inline(always)]
    #[cfg(test)]
    pub(in crate::vm::vm) const fn packed_enabled(self) -> bool {
        self.packed_enabled
    }

    #[inline(always)]
    pub(in crate::vm::vm) const fn packed_site_len(self) -> usize {
        self.packed_site_len
    }

    #[inline(always)]
    pub(in crate::vm::vm) const fn mixed_site_len(self) -> usize {
        self.mixed_site_len
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn packed_dispatch_code<'a>(self, fun: &'a Function) -> Option<PackedDispatchCode<'a>> {
        if !self.packed_enabled {
            return None;
        }
        let words = fun.code32.as_deref()?;
        debug_assert_eq!(self.packed_len, Some(words.len()));
        Some(PackedDispatchCode {
            words,
            decoded: fun.bc32_decoded.as_deref(),
        })
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn dispatch_mode<'a>(self, fun: &'a Function) -> RuntimeDispatchMode<'a> {
        match self.packed_dispatch_code(fun) {
            Some(code) => RuntimeDispatchMode::Packed(code),
            None => RuntimeDispatchMode::Opcode,
        }
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn frame_dispatch_plan<'a>(self, fun: &'a Function) -> FrameDispatchPlan<'a> {
        FrameDispatchPlan {
            fun,
            sites: self,
            mode: self.dispatch_mode(fun),
        }
    }
}

fn function_supports_packed_dispatch(fun: &Function) -> bool {
    if !fun.named_param_layout.is_empty() {
        return false;
    }
    let Some(code32) = fun.code32.as_ref() else {
        return false;
    };
    if fun.bc32_decoded.is_some() {
        return true;
    }
    !code32.iter().any(|word| bc32::tag_of(*word) == bc32::TAG_REG_EXT)
}

#[derive(Clone)]
pub(in crate::vm::vm) struct FunctionMetadataCache {
    pub(super) func_key: usize,
    pub(super) dispatch_sites: Option<RuntimeDispatchSites>,
    pub(super) region_plan: Option<Arc<RegionPlan>>,
    pub(super) runtime_plan: Option<FunctionRuntimePlan>,
}

impl FunctionMetadataCache {
    #[inline]
    pub(in crate::vm::vm) fn new() -> Self {
        Self {
            func_key: 0,
            dispatch_sites: None,
            region_plan: None,
            runtime_plan: None,
        }
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn clear(&mut self) {
        self.func_key = 0;
        self.dispatch_sites = None;
        self.region_plan = None;
        self.runtime_plan = None;
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn dispatch_sites_for_function(&mut self, fun: &Function) -> RuntimeDispatchSites {
        self.prepare_key(fun);
        if let Some(dispatch_sites) = self.dispatch_sites {
            return dispatch_sites;
        }
        let dispatch_sites = RuntimeDispatchSites::from_function(fun);
        self.dispatch_sites = Some(dispatch_sites);
        dispatch_sites
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn region_plan_for_function(&mut self, fun: &Function) -> Option<Arc<RegionPlan>> {
        self.prepare_key(fun);
        if let Some(ref region_plan) = self.region_plan {
            return Some(Arc::clone(region_plan));
        }
        let region_plan = fun.analysis.as_ref().map(|analysis| Arc::clone(&analysis.region_plan));
        self.region_plan = region_plan.clone();
        region_plan
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn runtime_plan_for_function(&mut self, fun: &Function) -> FunctionRuntimePlan {
        self.prepare_key(fun);
        if let Some(ref runtime_plan) = self.runtime_plan {
            return runtime_plan.clone();
        }
        let dispatch_sites = self.dispatch_sites_for_function(fun);
        let region_plan = self.region_plan_for_function(fun);
        let runtime_plan = FunctionRuntimePlan::from_parts(fun, dispatch_sites, region_plan);
        self.runtime_plan = Some(runtime_plan.clone());
        runtime_plan
    }

    #[inline(always)]
    fn prepare_key(&mut self, fun: &Function) {
        let func_key = fun as *const Function as usize;
        if self.func_key != func_key {
            self.func_key = func_key;
            self.dispatch_sites = None;
            self.region_plan = None;
            self.runtime_plan = None;
        }
    }
}

pub(in crate::vm::vm) struct RuntimeCacheStore {
    pub(super) sites: InstructionSiteCaches,
    pub(super) packed_hot_key: usize,
    pub(super) quickening_key: usize,
    pub(super) metadata: FunctionMetadataCache,
}

impl RuntimeCacheStore {
    #[inline]
    pub(in crate::vm::vm) fn new() -> Self {
        Self {
            sites: InstructionSiteCaches::new(),
            packed_hot_key: 0,
            quickening_key: 0,
            metadata: FunctionMetadataCache::new(),
        }
    }

    #[inline]
    pub(in crate::vm::vm) fn vm_caches(&mut self) -> VmCaches<'_> {
        self.sites.vm_caches()
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_function_runtime(&mut self, fun: &Function) -> FunctionRuntimePlan {
        let runtime = self.metadata.runtime_plan_for_function(fun);
        self.prepare_packed_hot_for_function(runtime.func_key);
        self.prepare_quickening_for_function(runtime.func_key);
        runtime
    }

    #[inline(always)]
    fn prepare_packed_hot_for_function(&mut self, func_key: usize) {
        if self.packed_hot_key != func_key {
            self.sites.packed_hot.clear();
            self.packed_hot_key = func_key;
        }
    }

    #[inline(always)]
    fn prepare_quickening_for_function(&mut self, func_key: usize) {
        if self.quickening_key != func_key {
            self.sites.quickening.clear();
            self.quickening_key = func_key;
        }
    }
}

#[derive(Clone)]
pub(super) struct InstructionSiteCaches {
    pub(super) access_ic: Vec<Option<AccessIc>>,
    pub(super) index_ic: Vec<Option<IndexIc>>,
    pub(super) global_ic: Vec<Option<GlobalEntry>>,
    pub(super) call_ic: Vec<Option<CallIc>>,
    pub(super) for_range: Vec<Option<ForRangeState>>,
    pub(super) packed_hot: Vec<Option<PackedHotEntry>>,
    pub(super) quickening: Vec<QuickeningSite>,
}

impl InstructionSiteCaches {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            access_ic: Vec::new(),
            index_ic: Vec::new(),
            global_ic: Vec::new(),
            call_ic: Vec::new(),
            for_range: Vec::new(),
            packed_hot: Vec::new(),
            quickening: Vec::new(),
        }
    }

    #[inline(always)]
    pub(super) fn clear(&mut self) {
        self.access_ic.clear();
        self.index_ic.clear();
        self.global_ic.clear();
        self.call_ic.clear();
        self.for_range.clear();
        self.packed_hot.clear();
        self.quickening.clear();
    }

    #[inline]
    pub(super) fn vm_caches(&mut self) -> VmCaches<'_> {
        VmCaches {
            access_ic: &mut self.access_ic,
            index_ic: &mut self.index_ic,
            global_ic: &mut self.global_ic,
            call_ic: &mut self.call_ic,
            for_range: &mut self.for_range,
            packed_hot: &mut self.packed_hot,
            quickening: &mut self.quickening,
        }
    }
}

pub(in crate::vm::vm) struct VmCaches<'a> {
    pub(in crate::vm::vm) access_ic: &'a mut Vec<Option<AccessIc>>,
    pub(in crate::vm::vm) index_ic: &'a mut Vec<Option<IndexIc>>,
    pub(in crate::vm::vm) global_ic: &'a mut Vec<Option<GlobalEntry>>,
    pub(in crate::vm::vm) call_ic: &'a mut Vec<Option<CallIc>>,
    pub(in crate::vm::vm) for_range: &'a mut Vec<Option<ForRangeState>>,
    pub(in crate::vm::vm) packed_hot: &'a mut Vec<Option<PackedHotEntry>>,
    pub(in crate::vm::vm) quickening: &'a mut Vec<QuickeningSite>,
}

impl VmCaches<'_> {
    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_opcode_sites(&mut self, dispatch_sites: RuntimeDispatchSites) {
        let opcode_len = dispatch_sites.opcode_len();
        if self.access_ic.len() < opcode_len {
            self.access_ic.resize(opcode_len, None);
        }
        if self.index_ic.len() < opcode_len {
            self.index_ic.resize(opcode_len, None);
        }
        if self.global_ic.len() < opcode_len {
            self.global_ic.resize(opcode_len, None);
        }
        if self.call_ic.len() < opcode_len {
            self.call_ic.resize(opcode_len, None);
        }
        if self.for_range.len() < opcode_len {
            self.for_range.resize(opcode_len, None);
        }
        if self.quickening.len() < opcode_len {
            self.quickening.resize(opcode_len, Default::default());
        }
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_packed_sites(&mut self, dispatch_sites: RuntimeDispatchSites) {
        let code32_len = dispatch_sites.packed_site_len();
        let mixed_site_len = dispatch_sites.mixed_site_len();
        if self.access_ic.len() < mixed_site_len {
            self.access_ic.resize(mixed_site_len, None);
        }
        if self.index_ic.len() < code32_len {
            self.index_ic.resize(code32_len, None);
        }
        if self.global_ic.len() < code32_len {
            self.global_ic.resize(code32_len, None);
        }
        if self.call_ic.len() < code32_len {
            self.call_ic.resize(code32_len, None);
        }
        if self.for_range.len() < mixed_site_len {
            self.for_range.resize(mixed_site_len, None);
        }
        if self.packed_hot.len() < code32_len {
            self.packed_hot.resize(code32_len, None);
        }
    }
}

impl FrameDispatchPlan<'_> {
    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_packed_sites(self, caches: &mut VmCaches<'_>) {
        caches.prepare_packed_sites(self.sites);
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_opcode_sites(self, caches: &mut VmCaches<'_>) {
        caches.prepare_opcode_sites(self.sites);
    }
}

impl ClosureFastCache {
    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_function_runtime(&mut self, fun: &Function) -> FunctionRuntimePlan {
        let func_key = fun as *const Function as usize;
        if self.prepared_func_key != func_key {
            self.clear_function_sites_for_key(func_key);
        }
        let runtime = self.metadata.runtime_plan_for_function(fun);
        self.prepare_function_sites(runtime.func_key, runtime.dispatch_sites);
        self.prepare_packed_hot_for_function(runtime.func_key);
        runtime
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_cached_function_runtime(
        &mut self,
        runtime: &FunctionRuntimePlan,
    ) -> FunctionRuntimePlan {
        if self.prepared_func_key != runtime.func_key {
            self.clear_function_sites_for_key(runtime.func_key);
        }
        self.prepare_function_sites(runtime.func_key, runtime.dispatch_sites);
        self.prepare_packed_hot_for_function(runtime.func_key);
        runtime.clone()
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_packed_hot_for_function(&mut self, func_key: usize) {
        if self.packed_hot_key != func_key {
            self.sites.packed_hot.clear();
            self.packed_hot_key = func_key;
        }
    }

    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_function_sites(&mut self, func_key: usize, dispatch_sites: RuntimeDispatchSites) {
        if self.prepared_func_key != func_key {
            self.clear_function_sites_for_key(func_key);
        }
        let opcode_len = dispatch_sites.opcode_len();
        if self.prepared_opcode_len < opcode_len {
            self.grow_function_sites(opcode_len);
            self.prepared_opcode_len = opcode_len;
        }
    }

    #[inline(always)]
    fn clear_function_sites_for_key(&mut self, func_key: usize) {
        self.sites.clear();
        self.packed_hot_key = 0;
        self.metadata.clear();
        self.prepared_func_key = func_key;
        self.prepared_opcode_len = 0;
    }

    #[inline(always)]
    fn grow_function_sites(&mut self, code_len: usize) {
        if self.sites.access_ic.len() < code_len {
            self.sites.access_ic.resize(code_len, None);
        }
        if self.sites.index_ic.len() < code_len {
            self.sites.index_ic.resize(code_len, None);
        }
        if self.sites.global_ic.len() < code_len {
            self.sites.global_ic.resize(code_len, None);
        }
        if self.sites.call_ic.len() < code_len {
            self.sites.call_ic.resize(code_len, None);
        }
        if self.sites.for_range.len() < code_len {
            self.sites.for_range.resize(code_len, None);
        }
        if self.sites.quickening.len() < code_len {
            self.sites.quickening.resize(code_len, Default::default());
        }
    }
}

impl Vm {
    #[inline(always)]
    pub(in crate::vm::vm) fn prepare_top_level_runtime(&mut self, fun: &Function) -> FunctionRuntimePlan {
        self.runtime_caches.prepare_function_runtime(fun)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::alloc::AllocationRegion;
    use crate::vm::analysis::FunctionAnalysis;

    fn function_with_region_plan(values: Vec<AllocationRegion>) -> Function {
        Function {
            consts: Vec::new(),
            code: Vec::new(),
            n_regs: 0,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: Some(FunctionAnalysis {
                region_plan: Arc::new(RegionPlan {
                    values,
                    return_region: AllocationRegion::ThreadLocal,
                }),
                ..FunctionAnalysis::default()
            }),
        }
    }

    #[test]
    fn opcode_site_prepare_sizes_quickening_sites() {
        let mut access_ic = Vec::new();
        let mut index_ic = Vec::new();
        let mut global_ic = Vec::new();
        let mut call_ic = Vec::new();
        let mut for_range = Vec::new();
        let mut packed_hot = Vec::new();
        let mut quickening = Vec::new();
        let mut caches = VmCaches {
            access_ic: &mut access_ic,
            index_ic: &mut index_ic,
            global_ic: &mut global_ic,
            call_ic: &mut call_ic,
            for_range: &mut for_range,
            packed_hot: &mut packed_hot,
            quickening: &mut quickening,
        };

        caches.prepare_opcode_sites(RuntimeDispatchSites::new(3, None));

        assert_eq!(caches.access_ic.len(), 3);
        assert_eq!(caches.quickening.len(), 3);
    }

    #[test]
    fn closure_function_site_prepare_sizes_quickening_sites() {
        let mut cache = ClosureFastCache::new();

        cache.prepare_function_sites(42, RuntimeDispatchSites::new(5, None));

        assert_eq!(cache.sites.access_ic.len(), 5);
        assert_eq!(cache.sites.quickening.len(), 5);
    }

    #[test]
    fn closure_function_site_prepare_invalidates_cached_region_plan_on_key_change() {
        let mut cache = ClosureFastCache::new();
        let first = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        let second = function_with_region_plan(vec![AllocationRegion::Heap]);

        cache.prepare_function_sites(
            1,
            RuntimeDispatchSites::new(first.code.len(), first.code32.as_ref().map(Vec::len)),
        );
        let first_plan = cache
            .metadata
            .region_plan_for_function(&first)
            .expect("first region plan");
        assert_eq!(first_plan.region_for(0), AllocationRegion::ThreadLocal);

        cache.prepare_function_sites(
            2,
            RuntimeDispatchSites::new(second.code.len(), second.code32.as_ref().map(Vec::len)),
        );
        let second_plan = cache
            .metadata
            .region_plan_for_function(&second)
            .expect("second region plan");
        assert_eq!(second_plan.region_for(0), AllocationRegion::Heap);
    }

    #[test]
    fn closure_function_runtime_prepare_sets_sites_packed_hot_and_region_plan() {
        let mut cache = ClosureFastCache::new();
        let fun = function_with_region_plan(vec![AllocationRegion::Heap]);
        let func_key = &fun as *const Function as usize;

        let runtime = cache.prepare_function_runtime(&fun);

        assert_eq!(cache.prepared_func_key, func_key);
        assert_eq!(cache.packed_hot_key, func_key);
        assert_eq!(runtime.func_key, func_key);
        assert_eq!(runtime.reg_count, 0);
        assert_eq!(runtime.dispatch_sites.opcode_len(), 0);
        assert_eq!(runtime.dispatch_sites.packed_len(), None);
        assert!(!runtime.dispatch_sites.packed_enabled());
        assert_eq!(
            runtime.region_plan.expect("region plan").region_for(0),
            AllocationRegion::Heap
        );
    }

    #[test]
    fn closure_function_runtime_prepare_caches_dispatch_sites_until_key_change() {
        let mut cache = ClosureFastCache::new();
        let mut first = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        first.code32 = Some(vec![1, 2, 3]);
        let mut second = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        second.code32 = Some(vec![1, 2]);

        let first_runtime = cache.prepare_function_runtime(&first);
        let cached_first = cache.metadata.dispatch_sites.expect("cached dispatch sites");
        let cached_first_runtime = cache.metadata.runtime_plan.as_ref().expect("cached runtime plan");
        assert_eq!(first_runtime.dispatch_sites, cached_first);
        assert_eq!(cached_first_runtime.dispatch_sites, cached_first);
        assert_eq!(cached_first.packed_len(), Some(3));

        let second_runtime = cache.prepare_function_runtime(&second);

        assert_eq!(second_runtime.dispatch_sites.packed_len(), Some(2));
        assert_eq!(
            cache
                .metadata
                .dispatch_sites
                .expect("cached dispatch sites")
                .packed_len(),
            Some(2)
        );
        assert_eq!(
            cache
                .metadata
                .runtime_plan
                .as_ref()
                .expect("cached runtime plan")
                .dispatch_sites
                .packed_len(),
            Some(2)
        );
    }

    #[test]
    fn top_level_runtime_prepare_sets_cache_keys_and_returns_region_plan() {
        let mut vm = Vm::new();
        let fun = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        let func_key = &fun as *const Function as usize;

        let runtime = vm.prepare_top_level_runtime(&fun);

        assert_eq!(vm.runtime_caches.packed_hot_key, func_key);
        assert_eq!(vm.runtime_caches.quickening_key, func_key);
        assert_eq!(vm.runtime_caches.metadata.func_key, func_key);
        assert!(vm.runtime_caches.metadata.region_plan.is_some());
        assert!(vm.runtime_caches.metadata.runtime_plan.is_some());
        assert_eq!(runtime.func_key, func_key);
        assert_eq!(runtime.reg_count, 0);
        assert_eq!(runtime.dispatch_sites.opcode_len(), 0);
        assert_eq!(runtime.dispatch_sites.packed_len(), None);
        assert!(!runtime.dispatch_sites.packed_enabled());
        assert_eq!(
            runtime.region_plan.expect("region plan").region_for(0),
            AllocationRegion::ThreadLocal
        );
    }

    #[test]
    fn top_level_runtime_prepare_clears_function_keyed_site_caches_on_key_change() {
        let mut vm = Vm::new();
        let first = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        let second = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        let second_key = &second as *const Function as usize;

        let _ = vm.prepare_top_level_runtime(&first);
        vm.runtime_caches.sites.packed_hot.push(None);
        vm.runtime_caches.sites.quickening.push(Default::default());

        let _ = vm.prepare_top_level_runtime(&second);

        assert_eq!(vm.runtime_caches.packed_hot_key, second_key);
        assert_eq!(vm.runtime_caches.quickening_key, second_key);
        assert!(vm.runtime_caches.sites.packed_hot.is_empty());
        assert!(vm.runtime_caches.sites.quickening.is_empty());
    }

    #[test]
    fn top_level_runtime_prepare_caches_dispatch_sites_until_key_change() {
        let mut vm = Vm::new();
        let mut first = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        first.code32 = Some(vec![1, 2, 3]);
        let mut second = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        second.code32 = Some(vec![1, 2]);

        let first_runtime = vm.prepare_top_level_runtime(&first);
        let cached_first = vm
            .runtime_caches
            .metadata
            .dispatch_sites
            .expect("cached dispatch sites");
        let cached_first_runtime = vm
            .runtime_caches
            .metadata
            .runtime_plan
            .as_ref()
            .expect("cached runtime plan");
        assert_eq!(first_runtime.dispatch_sites, cached_first);
        assert_eq!(cached_first_runtime.dispatch_sites, cached_first);
        assert_eq!(cached_first.packed_len(), Some(3));

        let repeated_runtime = vm.prepare_top_level_runtime(&first);
        assert_eq!(repeated_runtime.dispatch_sites, cached_first);
        assert_eq!(
            vm.runtime_caches
                .metadata
                .dispatch_sites
                .expect("cached dispatch sites")
                .packed_len(),
            Some(3)
        );
        assert_eq!(
            vm.runtime_caches
                .metadata
                .runtime_plan
                .as_ref()
                .expect("cached runtime plan")
                .dispatch_sites
                .packed_len(),
            Some(3)
        );

        let second_runtime = vm.prepare_top_level_runtime(&second);

        assert_eq!(second_runtime.dispatch_sites.packed_len(), Some(2));
        assert_eq!(
            vm.runtime_caches
                .metadata
                .dispatch_sites
                .expect("cached dispatch sites")
                .packed_len(),
            Some(2)
        );
        assert_eq!(
            vm.runtime_caches
                .metadata
                .runtime_plan
                .as_ref()
                .expect("cached runtime plan")
                .dispatch_sites
                .packed_len(),
            Some(2)
        );
    }

    #[test]
    fn top_level_runtime_prepare_caches_region_plan_until_key_change() {
        let mut vm = Vm::new();
        let first = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        let second = function_with_region_plan(vec![AllocationRegion::Heap]);

        let first_runtime = vm.prepare_top_level_runtime(&first);
        let cached_first = vm
            .runtime_caches
            .metadata
            .region_plan
            .as_ref()
            .expect("cached region plan");
        let cached_first_runtime = vm
            .runtime_caches
            .metadata
            .runtime_plan
            .as_ref()
            .expect("cached runtime plan");
        assert_eq!(
            first_runtime.region_plan.expect("first runtime plan").region_for(0),
            AllocationRegion::ThreadLocal
        );
        assert_eq!(cached_first.region_for(0), AllocationRegion::ThreadLocal);
        assert_eq!(
            cached_first_runtime
                .region_plan
                .as_ref()
                .expect("cached runtime region plan")
                .region_for(0),
            AllocationRegion::ThreadLocal
        );

        let repeated_runtime = vm.prepare_top_level_runtime(&first);
        assert_eq!(
            repeated_runtime
                .region_plan
                .expect("repeated runtime plan")
                .region_for(0),
            AllocationRegion::ThreadLocal
        );
        assert_eq!(
            vm.runtime_caches
                .metadata
                .region_plan
                .as_ref()
                .expect("cached region plan")
                .region_for(0),
            AllocationRegion::ThreadLocal
        );
        assert_eq!(
            vm.runtime_caches
                .metadata
                .runtime_plan
                .as_ref()
                .expect("cached runtime plan")
                .region_plan
                .as_ref()
                .expect("cached runtime region plan")
                .region_for(0),
            AllocationRegion::ThreadLocal
        );

        let second_runtime = vm.prepare_top_level_runtime(&second);

        assert_eq!(
            second_runtime.region_plan.expect("second runtime plan").region_for(0),
            AllocationRegion::Heap
        );
        assert_eq!(
            vm.runtime_caches
                .metadata
                .region_plan
                .as_ref()
                .expect("cached region plan")
                .region_for(0),
            AllocationRegion::Heap
        );
        assert_eq!(
            vm.runtime_caches
                .metadata
                .runtime_plan
                .as_ref()
                .expect("cached runtime plan")
                .region_plan
                .as_ref()
                .expect("cached runtime region plan")
                .region_for(0),
            AllocationRegion::Heap
        );
    }

    #[test]
    fn function_runtime_plan_records_packed_code_len() {
        let mut fun = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        fun.code32 = Some(vec![1, 2, 3]);

        let runtime = FunctionRuntimePlan::from_function(&fun, None);

        assert_eq!(runtime.dispatch_sites.opcode_len(), 0);
        assert_eq!(runtime.dispatch_sites.packed_len(), Some(3));
        assert_eq!(runtime.dispatch_sites.packed_site_len(), 3);
        assert_eq!(runtime.dispatch_sites.mixed_site_len(), 3);
        assert!(runtime.dispatch_sites.packed_enabled());
        assert_eq!(
            runtime
                .dispatch_sites
                .packed_dispatch_code(&fun)
                .expect("packed dispatch code")
                .words,
            &[1, 2, 3]
        );
        match runtime.dispatch_sites.dispatch_mode(&fun) {
            RuntimeDispatchMode::Packed(code) => assert_eq!(code.words, &[1, 2, 3]),
            RuntimeDispatchMode::Opcode => panic!("expected packed dispatch mode"),
        }
        match runtime.dispatch_sites.frame_dispatch_plan(&fun).mode() {
            RuntimeDispatchMode::Packed(code) => assert_eq!(code.words, &[1, 2, 3]),
            RuntimeDispatchMode::Opcode => panic!("expected packed dispatch plan"),
        }
    }

    #[test]
    fn function_runtime_plan_disables_packed_dispatch_for_named_params() {
        let mut fun = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        fun.code32 = Some(vec![1, 2, 3]);
        fun.named_param_layout.push(crate::vm::bytecode::NamedParamLayoutEntry {
            name_const_idx: 0,
            dest_reg: 0,
            default_index: None,
        });

        let runtime = FunctionRuntimePlan::from_function(&fun, None);

        assert_eq!(runtime.dispatch_sites.packed_len(), Some(3));
        assert!(!runtime.dispatch_sites.packed_enabled());
        assert!(runtime.dispatch_sites.packed_dispatch_code(&fun).is_none());
        assert!(matches!(
            runtime.dispatch_sites.dispatch_mode(&fun),
            RuntimeDispatchMode::Opcode
        ));
    }

    #[test]
    fn function_runtime_plan_requires_decoded_table_for_reg_ext_dispatch() {
        let mut fun = function_with_region_plan(vec![AllocationRegion::ThreadLocal]);
        fun.code32 = Some(vec![(bc32::TAG_REG_EXT as u32) << 24, 1]);

        let runtime = FunctionRuntimePlan::from_function(&fun, None);

        assert_eq!(runtime.dispatch_sites.packed_len(), Some(2));
        assert!(!runtime.dispatch_sites.packed_enabled());
        assert!(runtime.dispatch_sites.packed_dispatch_code(&fun).is_none());
        assert!(matches!(
            runtime.dispatch_sites.dispatch_mode(&fun),
            RuntimeDispatchMode::Opcode
        ));
    }
}
