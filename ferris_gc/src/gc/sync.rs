use alloc::alloc::{alloc, dealloc, Layout};
use core::mem::transmute;
use core::cell::{Cell, RefCell};
#[cfg(feature = "no_std")]
use hashbrown::HashMap;
#[cfg(not(feature = "no_std"))]
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use crate::gc::{Finalize, Trace};
use crate::basic_gc_strategy::{basic_gc_strategy_start, BASIC_STRATEGY_GLOBAL_GC};
use std::hash::Hasher;

pub type OptGc<T> = Option<Gc<T>>;
pub type OptGcCell<T> = Option<Gc<T>>;

struct GcInfo {
    root_ref_count: AtomicUsize,
}

impl GcInfo {
    fn new() -> GcInfo {
        GcInfo {
            root_ref_count: AtomicUsize::new(0),
        }
    }
}

pub struct GcPtr<T> where T: 'static + Sized + Trace {
    info: GcInfo,
    t: T,
}

impl<T> GcPtr<T> where T: 'static + Sized + Trace {
    fn new(t: T) -> GcPtr<T> {
        GcPtr {
            info: GcInfo::new(),
            t: t,
        }
    }
}

impl<T> Deref for GcPtr<T> where T: 'static + Sized + Trace {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.t
    }
}

impl<T> DerefMut for GcPtr<T> where T: 'static + Sized + Trace {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.t
    }
}

impl<T> Trace for GcPtr<T> where T: Sized + Trace {
    fn is_root(&self) -> bool {
        unreachable!("is_root on GcPtr is unreachable !!");
    }

    fn reset_root(&self) {
        self.t.reset_root();
    }

    fn trace(&self) {
        self.info.root_ref_count.fetch_add(1, Ordering::AcqRel);
        self.t.trace();
    }

    fn reset(&self) {
        self.info.root_ref_count.fetch_sub(1, Ordering::AcqRel);
        self.t.reset();
    }

    fn is_traceable(&self) -> bool {
        self.info.root_ref_count.load(Ordering::Acquire) > 0
    }
}

impl<T> Trace for RefCell<GcPtr<T>> where T: Sized + Trace {
    fn is_root(&self) -> bool {
        unreachable!("is_root on GcPtr is unreachable !!");
    }

    fn reset_root(&self) {
        self.borrow().t.reset_root();
    }

    fn trace(&self) {
        self.borrow().info.root_ref_count.fetch_add(1, Ordering::AcqRel);
        self.borrow().t.trace();
    }

    fn reset(&self) {
        self.borrow().info.root_ref_count.fetch_sub(1, Ordering::AcqRel);
        self.borrow().t.reset();
    }

    fn is_traceable(&self) -> bool {
        self.borrow().info.root_ref_count.load(Ordering::Acquire) > 0
    }
}

impl<T> Finalize for RefCell<GcPtr<T>> where T: Sized + Trace {
    fn finalize(&self) {}
}

impl<T> Finalize for GcPtr<T> where T: Sized + Trace {
    fn finalize(&self) {}
}

pub struct GcInternal<T> where T: 'static + Sized + Trace {
    is_root: AtomicBool,
    ptr: *const GcPtr<T>,
}

impl<T> GcInternal<T> where T: 'static + Sized + Trace {
    fn new(ptr: *const GcPtr<T>) -> GcInternal<T> {
        GcInternal {
            is_root: AtomicBool::new(true),
            ptr: ptr,
        }
    }
}

impl<T> Trace for GcInternal<T> where T: Sized + Trace {
    fn is_root(&self) -> bool {
        self.is_root.load(Ordering::Acquire)
    }

    fn reset_root(&self) {
        if self.is_root.load(Ordering::Acquire) {
            self.is_root.store(false, Ordering::Release);
            unsafe {
                (*self.ptr).reset_root();
            }
        }
    }

    fn trace(&self) {
        unsafe {
            (*self.ptr).trace();
        }
    }

    fn reset(&self) {
        unsafe {
            (*self.ptr).reset();
        }
    }

    fn is_traceable(&self) -> bool {
        unsafe {
            (*self.ptr).is_traceable()
        }
    }
}

impl<T> Finalize for GcInternal<T> where T: Sized + Trace {
    fn finalize(&self) {}
}

pub struct Gc<T> where T: 'static + Sized + Trace {
    internal_ptr: *mut GcInternal<T>,
    ptr: *const GcPtr<T>,
}

unsafe impl<T> Sync for Gc<T> where T: 'static + Sized + Trace + Sync {}
unsafe impl<T> Send for Gc<T> where T: 'static + Sized + Trace + Send {}

impl<T> Deref for Gc<T> where T: 'static + Sized + Trace {
    type Target = GcPtr<T>;

    fn deref(&self) -> &Self::Target {
        unsafe {
            &(*self.ptr)
        }
    }
}

impl<T> Gc<T> where T: Sized + Trace {
    pub fn new<'a>(t: T) -> Gc<T> {
        basic_gc_strategy_start();
        let global_strategy = &(*GLOBAL_GC_STRATEGY);
        if !global_strategy.is_active() {
            global_strategy.start();
        }
        unsafe {
            (*GLOBAL_GC).create_gc(t)
        }
    }
}

impl<T> Clone for Gc<T> where T: 'static + Sized + Trace {
    fn clone(&self) -> Self {
        unsafe {
            (*GLOBAL_GC).clone_from_gc(self)
        }
    }

    fn clone_from(&mut self, source: &Self) {
        unsafe {
            (*self.internal_ptr).ptr = (*source.internal_ptr).ptr;
        }
    }
}

impl<T> Drop for Gc<T> where T: Sized + Trace {
    fn drop(&mut self) {
        unsafe {
            (*GLOBAL_GC).remove_tracer(self.internal_ptr);
        }
    }
}

impl<T> Trace for Gc<T> where T: Sized + Trace {
    fn is_root(&self) -> bool {
        unsafe {
            (*self.internal_ptr).is_root()
        }
    }

    fn reset_root(&self) {
        unsafe {
            (*self.internal_ptr).reset_root();
        }
    }

    fn trace(&self) {
        unsafe {
            (*self.ptr).trace();
        }
    }

    fn reset(&self) {
        unsafe {
            (*self.ptr).reset();
        }
    }

    fn is_traceable(&self) -> bool {
        unsafe {
            (*self.ptr).is_traceable()
        }
    }
}

impl<T> Finalize for Gc<T> where T: Sized + Trace {
    fn finalize(&self) {}
}

pub struct GcCellInternal<T> where T: 'static + Sized + Trace {
    is_root: AtomicBool,
    ptr: *const RefCell<GcPtr<T>>,
}

impl<T> GcCellInternal<T> where T: 'static + Sized + Trace {
    fn new(ptr: *const RefCell<GcPtr<T>>) -> GcCellInternal<T> {
        GcCellInternal {
            is_root: AtomicBool::new(true),
            ptr: ptr,
        }
    }
}

impl<T> Trace for GcCellInternal<T> where T: Sized + Trace {
    fn is_root(&self) -> bool {
        self.is_root.load(Ordering::Acquire)
    }

    fn reset_root(&self) {
        if self.is_root.load(Ordering::Acquire) {
            self.is_root.store(false, Ordering::Release);
            unsafe {
                (*self.ptr).borrow().reset_root();
            }
        }
    }

    fn trace(&self) {
        unsafe {
            (*self.ptr).borrow().trace();
        }
    }

    fn reset(&self) {
        unsafe {
            (*self.ptr).borrow().reset();
        }
    }

    fn is_traceable(&self) -> bool {
        unsafe {
            (*self.ptr).borrow().is_traceable()
        }
    }
}

impl<T> Finalize for GcCellInternal<T> where T: Sized + Trace {
    fn finalize(&self) {}
}

pub struct GcRefCell<T> where T: 'static + Sized + Trace {
    internal_ptr: *mut GcCellInternal<T>,
    ptr: *const RefCell<GcPtr<T>>,
}

unsafe impl<T> Sync for GcRefCell<T> where T: 'static + Sized + Trace + Sync {}
unsafe impl<T> Send for GcRefCell<T> where T: 'static + Sized + Trace + Send {}

impl<T> Drop for GcRefCell<T> where T: Sized + Trace {
    fn drop(&mut self) {
        unsafe {
            (*GLOBAL_GC).remove_tracer(self.internal_ptr);
        }
    }
}

impl<T> Deref for GcRefCell<T> where T: 'static + Sized + Trace {
    type Target = RefCell<GcPtr<T>>;

    fn deref(&self) -> &Self::Target {
        unsafe {
            &(*self.ptr)
        }
    }
}

impl<T> GcRefCell<T> where T: 'static + Sized + Trace {
    pub fn new<'a>(t: T) -> GcRefCell<T> {
        basic_gc_strategy_start();
        let global_strategy = &(*GLOBAL_GC_STRATEGY);
        if !global_strategy.is_active() {
            global_strategy.start();
        }
        unsafe {
            (*GLOBAL_GC).create_gc_cell(t)
        }
    }
}

impl<T> Clone for GcRefCell<T> where T: 'static + Sized + Trace {
    fn clone(&self) -> Self {
        let gc = unsafe {
            (*GLOBAL_GC).clone_from_gc_cell(self)
        };
        unsafe {
            (*gc.internal_ptr).ptr = (*self.internal_ptr).ptr;
            (*gc.internal_ptr).is_root.store(true, Ordering::Release);
        }
        gc
    }

    fn clone_from(&mut self, source: &Self) {
        unsafe {
            (*self.internal_ptr).ptr = (*source.internal_ptr).ptr;
        }
    }
}

impl<T> Trace for GcRefCell<T> where T: Sized + Trace {
    fn is_root(&self) -> bool {
        unsafe {
            (*self.internal_ptr).is_root()
        }
    }

    fn reset_root(&self) {
        unsafe {
            (*self.internal_ptr).reset_root();
        }
    }

    fn trace(&self) {
        unsafe {
            (*self.ptr).borrow().trace();
        }
    }

    fn reset(&self) {
        unsafe {
            (*self.ptr).borrow().reset();
        }
    }

    fn is_traceable(&self) -> bool {
        unsafe {
            (*self.ptr).borrow().is_traceable()
        }
    }
}

impl<T> Finalize for GcRefCell<T> where T: Sized + Trace {
    fn finalize(&self) {}
}

type GcObjMem = *mut u8;

pub struct GlobalGarbageCollector {
    mem_to_trc: RwLock<HashMap<usize, *const dyn Trace>>,
    trs: RwLock<HashMap<*const dyn Trace, (GcObjMem, Layout)>>,
    objs: Mutex<HashMap<*const dyn Trace, (GcObjMem, Layout)>>,
    fin: Mutex<HashMap<*const dyn Trace, *const dyn Finalize>>,
}

unsafe impl Sync for GlobalGarbageCollector {}

unsafe impl Send for GlobalGarbageCollector {}

impl GlobalGarbageCollector {
    fn new() -> GlobalGarbageCollector {
        GlobalGarbageCollector {
            mem_to_trc: RwLock::new(HashMap::new()),
            trs: RwLock::new(HashMap::new()),
            objs: Mutex::new(HashMap::new()),
            fin: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_objs(&self) -> &Mutex<HashMap<*const dyn Trace, (*mut u8, Layout)>> {
        &self.objs
    }

    unsafe fn create_gc<T>(&self, t: T) -> Gc<T>
        where T: Sized + Trace {
        let (gc_ptr, mem_info_gc_ptr) = self.alloc_mem::<GcPtr<T>>();
        let (gc_inter_ptr, mem_info_internal_ptr) = self.alloc_mem::<GcInternal<T>>();
        std::ptr::write(gc_ptr, GcPtr::new(t));
        std::ptr::write(gc_inter_ptr, GcInternal::new(gc_ptr));
        let gc = Gc {
            internal_ptr: gc_inter_ptr,
            ptr: gc_ptr,
        };
        (*(*gc.internal_ptr).ptr).reset_root();
        let mut mem_to_trc = self.mem_to_trc.write().unwrap();
        let mut trs = self.trs.write().unwrap();
        let mut objs = self.objs.lock().unwrap();
        let mut fin = self.fin.lock().unwrap();
        mem_to_trc.insert(gc_inter_ptr as usize, gc_inter_ptr);
        trs.insert(gc_inter_ptr, mem_info_internal_ptr);
        objs.insert(gc_ptr, mem_info_gc_ptr);
        fin.insert(gc_ptr, (*gc_ptr).t.as_finalize());
        gc
    }

    unsafe fn clone_from_gc<T>(&self, gc: &Gc<T>) -> Gc<T> where T: Sized + Trace {
        let (gc_inter_ptr, mem_info_internal_ptr) = self.alloc_mem::<GcInternal<T>>();
        std::ptr::write(gc_inter_ptr, GcInternal::new(gc.ptr));
        let gc = Gc {
            internal_ptr: gc_inter_ptr,
            ptr: gc.ptr,
        };
        (*(*gc.internal_ptr).ptr).reset_root();
        let mut mem_to_trc = self.mem_to_trc.write().unwrap();
        let mut trs = self.trs.write().unwrap();
        mem_to_trc.insert(gc_inter_ptr as usize, gc_inter_ptr);
        trs.insert(gc_inter_ptr, mem_info_internal_ptr);
        gc
    }

    unsafe fn create_gc_cell<T>(&self, t: T) -> GcRefCell<T> where T: Sized + Trace {
        let (gc_ptr, mem_info_gc_ptr) = self.alloc_mem::<RefCell<GcPtr<T>>>();
        let (gc_cell_inter_ptr, mem_info_internal_ptr) = self.alloc_mem::<GcCellInternal<T>>();
        std::ptr::write(gc_ptr, RefCell::new(GcPtr::new(t)));
        std::ptr::write(gc_cell_inter_ptr, GcCellInternal::new(gc_ptr));
        let gc = GcRefCell {
            internal_ptr: gc_cell_inter_ptr,
            ptr: gc_ptr,
        };
        (*(*gc.internal_ptr).ptr).reset_root();
        let mut mem_to_trc = self.mem_to_trc.write().unwrap();
        let mut trs = self.trs.write().unwrap();
        let mut objs = self.objs.lock().unwrap();
        let mut fin = self.fin.lock().unwrap();
        mem_to_trc.insert(gc_cell_inter_ptr as usize, gc_cell_inter_ptr);
        trs.insert(gc_cell_inter_ptr, mem_info_internal_ptr);
        objs.insert(gc_ptr, mem_info_gc_ptr);
        fin.insert(gc_ptr, (*(*gc_ptr).as_ptr()).t.as_finalize());
        gc
    }

    unsafe fn clone_from_gc_cell<T>(&self, gc: &GcRefCell<T>) -> GcRefCell<T> where T: Sized + Trace {
        let (gc_inter_ptr, mem_info) = self.alloc_mem::<GcCellInternal<T>>();
        std::ptr::write(gc_inter_ptr, GcCellInternal::new(gc.ptr));
        let gc = GcRefCell {
            internal_ptr: gc_inter_ptr,
            ptr: gc.ptr,
        };
        (*(*gc.internal_ptr).ptr).reset_root();
        let mut mem_to_trc = self.mem_to_trc.write().unwrap();
        let mut trs = self.trs.write().unwrap();
        mem_to_trc.insert(gc_inter_ptr as usize, gc_inter_ptr);
        trs.insert(gc_inter_ptr, mem_info);
        gc
    }

    unsafe fn alloc_mem<T>(&self) -> (*mut T, (GcObjMem, Layout)) where T: Sized {
        let layout = Layout::new::<T>();
        let mem = alloc(layout);
        let type_ptr: *mut T = mem as *mut _;
        (type_ptr, (mem, layout))
    }

    unsafe fn remove_tracer(&self, tracer: *const dyn Trace) {
        let mut mem_to_trc = self.mem_to_trc.write().unwrap();
        let mut trs = self.trs.write().unwrap();
        let (tracer_thin_ptr, _) = unsafe { transmute::<_, (*const (), *const ())>(tracer) };
        let tracer = &mem_to_trc.remove(&(tracer_thin_ptr as usize)).unwrap();
        let del = trs.remove(&tracer).unwrap();
        dealloc(del.0, del.1);
    }

    pub unsafe fn collect(&self) {
        let mut trs = self.trs.write().unwrap();
        for (gc_info, _) in &*trs {
            let tracer = &(**gc_info);
            if tracer.is_root() {
                tracer.trace();
            }
        }
        let mut collected_tracers = Vec::new();
        for (gc_info, _) in &*trs {
            let tracer = &(**gc_info);
            if !tracer.is_traceable() {
                collected_tracers.push(*gc_info);
            }
        }
        for tracer_ptr in collected_tracers {
            let del = (&*trs)[&tracer_ptr];
            dealloc(del.0, del.1);
            trs.remove(&tracer_ptr);
        }
        let mut collected_objects = Vec::new();
        let mut objs = self.objs.lock().unwrap();
        for (gc_info, _) in &*objs {
            let obj = &(**gc_info);
            if !obj.is_traceable() {
                collected_objects.push(*gc_info);
            }
        }
        for (gc_info, _) in &*trs {
            let tracer = &(**gc_info);
            tracer.reset();
        }
        let mut fin = self.fin.lock().unwrap();
        let _clone_collected_objects = collected_objects.clone();
        for col in collected_objects {
            let del = (&*objs)[&col];
            let finilizer = (&*fin)[&col];
            (*finilizer).finalize();
            dealloc(del.0, del.1);
            objs.remove(&col);
            fin.remove(&col);
        }
    }

    unsafe fn collect_all(&self) {
        let mut collected_tracers: Vec<*const dyn Trace> = Vec::new();
        let mut trs = self.trs.write().unwrap();
        for (gc_info, _) in &*trs {
            collected_tracers.push(*gc_info);
        }
        let mut collected_objects: Vec<*const dyn Trace> = Vec::new();
        let mut objs = self.objs.lock().unwrap();
        for (gc_info, _) in &*objs {
            collected_objects.push(*gc_info);
        }
        for tracer_ptr in collected_tracers {
            let del = (&*trs)[&tracer_ptr];
            dealloc(del.0, del.1);
            trs.remove(&tracer_ptr);
        }
        let mut fin = self.fin.lock().unwrap();
        for col in collected_objects {
            let del = (&*objs)[&col];
            let finilizer = (&*fin)[&col];
            (*finilizer).finalize();
            dealloc(del.0, del.1);
            objs.remove(&col);
            fin.remove(&col);
        }
    }
}

pub type StartGlobalStrategyFn = Box<dyn FnMut(&'static GlobalGarbageCollector, &'static AtomicBool) -> Option<JoinHandle<()>>>;
pub type StopGlobalStrategyFn = Box<dyn FnMut(&'static GlobalGarbageCollector)>;

pub struct GlobalStrategy {
    gc: Cell<&'static GlobalGarbageCollector>,
    is_active: AtomicBool,
    start_func: Mutex<StartGlobalStrategyFn>,
    stop_func: Mutex<StopGlobalStrategyFn>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

unsafe impl Sync for GlobalStrategy {}
unsafe impl Send for GlobalStrategy {}

impl GlobalStrategy {
    fn new<StartFn, StopFn>(gc: &'static GlobalGarbageCollector, start_fn: StartFn, stop_fn: StopFn) -> GlobalStrategy
        where StartFn: 'static + FnMut(&'static GlobalGarbageCollector, &'static AtomicBool) -> Option<JoinHandle<()>>,
              StopFn: 'static + FnMut(&'static GlobalGarbageCollector) {
        GlobalStrategy {
            gc: Cell::new(gc),
            is_active: AtomicBool::new(false),
            start_func: Mutex::new(Box::new(start_fn)),
            stop_func: Mutex::new(Box::new(stop_fn)),
            join_handle: Mutex::new(None),
        }
    }

    pub fn change_strategy<StartFn, StopFn>(&self, start_fn: StartFn, stop_fn: StopFn)
        where StartFn: 'static + FnMut(&'static GlobalGarbageCollector, &'static AtomicBool) -> Option<JoinHandle<()>>,
              StopFn: 'static + FnMut(&'static GlobalGarbageCollector) {
        let mut start_func = self.start_func.lock().unwrap();
        let mut stop_func = self.stop_func.lock().unwrap();
        if self.is_active() {
            self.stop();
        }
        *start_func = Box::new(start_fn);
        *stop_func = Box::new(stop_fn);
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Acquire)
    }

    pub fn start(&'static self) {
        self.is_active.store(true, Ordering::Release);
        let mut start_func = self.start_func.lock().unwrap();
        let mut join_handle = self.join_handle.lock().unwrap();
        *join_handle = (&mut *(start_func))(self.gc.get(), &self.is_active);
    }

    pub fn stop(&self) {
        self.is_active.store(false, Ordering::Release);
        let mut join_handle = self.join_handle.lock().unwrap();
        if let Some(join_handle) = join_handle.take() {
            join_handle.join().expect("GlobalStrategy::stop, GlobalStrategy Thread being joined has panicked !!");
        }
        let mut stop_func = self.stop_func.lock().unwrap();
        (&mut *(stop_func))(self.gc.get());
    }
}

impl Drop for GlobalStrategy {
    fn drop(&mut self) {
        self.is_active.store(false, Ordering::Release);
        let mut stop_func = self.stop_func.lock().unwrap();
        (&mut *(stop_func))(self.gc.get());
    }
}

lazy_static! {
    static ref GLOBAL_GC: GlobalGarbageCollector = {
        GlobalGarbageCollector::new()
    };
    pub static ref GLOBAL_GC_STRATEGY: GlobalStrategy = {
        let gc = &(*GLOBAL_GC);
        GlobalStrategy::new(gc,
            move |global_gc, _| {
                let mut basic_strategy_global_gc = BASIC_STRATEGY_GLOBAL_GC.write().unwrap();
                *basic_strategy_global_gc = Some(global_gc);
                None
            },
            move |_global_gc| {
                let mut basic_strategy_global_gc = BASIC_STRATEGY_GLOBAL_GC.write().unwrap();
                *basic_strategy_global_gc = None;
            })
    };
}

#[cfg(test)]
mod tests {
    use crate::gc::sync::{Gc, GLOBAL_GC};

    #[test]
    fn one_object() {
        let _one = Gc::new(1);
        unsafe { (*GLOBAL_GC).collect() };
        assert_eq!((*GLOBAL_GC).trs.read().unwrap().len(), 1);
    }

    #[test]
    fn gc_collect_one_from_one() {
        {
            let _one = Gc::new(1);
        }
        unsafe { (*GLOBAL_GC).collect() };
        assert_eq!((*GLOBAL_GC).trs.read().unwrap().len(), 0);
    }

    #[test]
    fn two_objects() {
        let mut one = Gc::new(1);
        one = Gc::new(2);
        unsafe { (*GLOBAL_GC).collect() };
        assert_eq!((*GLOBAL_GC).trs.read().unwrap().len(), 2);
    }

    #[test]
    fn gc_collect_one_from_two() {
        let mut one = Gc::new(1);
        one = Gc::new(2);
        unsafe { (*GLOBAL_GC).collect() };
        assert_eq!((*GLOBAL_GC).trs.read().unwrap().len(), 0);
    }

    #[test]
    fn gc_collect_two_from_two() {
        {
            let mut one = Gc::new(1);
            one = Gc::new(2);
        }
        unsafe { (*GLOBAL_GC).collect() };
        assert_eq!((*GLOBAL_GC).trs.read().unwrap().len(), 0);
    }
}
