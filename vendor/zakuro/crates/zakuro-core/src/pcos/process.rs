//! native-final: the game process's memory, information and debug calls.
//!
//! These are the supervisor calls the SDK makes to set the process up
//! (`nn::os` heap and linear heap, the GSP/HID/shared font memory blocks,
//! the resource limits, the srv: port) plus the clock, process information
//! and debug output. A full replay from boot made 52 of them and they were
//! the last calls that reached Zakuro's kernel; they are answered here.
//!
//! What they act on is the game's address space: the page table and the
//! physical allocator in `crate::memory`. That stays, as in every static
//! recompilation (N64Recomp keeps RDRAM, Unleashed Recompiled the Xbox 360's
//! address space): the recompiled code addresses memory by its original
//! addresses. It is plain host memory with a page table in front, not an
//! emulated MMU; nothing is translated per access beyond a table lookup.
//!
//! The answers are exactly the ones Zakuro's kernel gave, so replays and
//! save states are unchanged. `FATES_NATIVE_KERNEL=0` (development builds
//! only) sends them back to Zakuro's kernel for comparison.

use zakuro_common::memory_map::*;
use zakuro_common::result::errors;
use zakuro_cpu::Bus;

use super::stats;
use crate::kernel::object::{KObject, SharedMemory};
use crate::kernel::thread::ThreadStatus;
use crate::memory::{MemoryRegion, MemoryState, PhysicalBlock, Permission};
use crate::System;

// MemOp, as the SDK passes it.
const MEMOP_FREE: u32 = 1;
const MEMOP_RESERVE: u32 = 2;
const MEMOP_ALLOC: u32 = 3;
const MEMOP_MAP: u32 = 4;
const MEMOP_UNMAP: u32 = 5;
const MEMOP_PROTECT: u32 = 6;
const MEMOP_OP_MASK: u32 = 0xFF;
const MEMOP_LINEAR_FLAG: u32 = 0x1_0000;

/// whether these calls are answered here (always in the pure build).
fn enabled() -> bool {
    #[cfg(not(feature = "emulation"))]
    return true;
    #[cfg(feature = "emulation")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("FATES_NATIVE_KERNEL").map_or(true, |v| v != "0"))
    }
}

/// answers a memory, process, clock or debug call; false for any other.
pub fn svc(system: &mut System, number: u32) -> bool {
    if !enabled() {
        return false;
    }
    let name = match number {
        0x01 => {
            control_memory(system);
            "process.ControlMemory"
        }
        0x02 => {
            let info = system.memory.query(system.cpu.regs[2]);
            let regs = &mut system.cpu.regs;
            regs[0] = 0;
            regs[1] = info.base;
            regs[2] = info.size;
            regs[3] = info.permission;
            regs[4] = info.state;
            regs[5] = 0;
            "process.QueryMemory"
        }
        0x03 => {
            log::info!("the game called svcExitProcess");
            for thread in &mut system.kernel.threads {
                thread.status = ThreadStatus::Dead;
            }
            system.kernel.current_thread = None;
            system.kernel.reschedule_pending = true;
            system.exited = true;
            "process.ExitProcess"
        }
        // GetCurrentProcessorNumber: the application core
        0x11 => {
            system.cpu.regs[0] = 0;
            "process.GetCurrentProcessorNumber"
        }
        0x1E => {
            create_memory_block(system);
            "process.CreateMemoryBlock"
        }
        0x1F => {
            map_memory_block(system);
            "process.MapMemoryBlock"
        }
        0x20 => {
            let (handle, addr) = (system.cpu.regs[0], system.cpu.regs[1]);
            if let Some(object) = system.kernel.resolve(handle) {
                if let Some(KObject::SharedMemory(block)) = system.kernel.objects.get(object) {
                    let size = block.size;
                    system.memory.unmap(addr, size);
                }
            }
            system.cpu.regs[0] = 0;
            "process.UnmapMemoryBlock"
        }
        0x28 => {
            let tick = system.cpu.cycles;
            system.cpu.regs[0] = tick as u32;
            system.cpu.regs[1] = (tick >> 32) as u32;
            // the call itself takes this long on the console
            system.cpu.cycles += super::svc::SYSTEM_TICK_COST;
            "process.GetSystemTick"
        }
        // online: GetHandleInfo(handle, type). Only type 0 of the process,
        // its creation time, is asked: nn::os::GetCreationTime, for local
        // wireless's scrambled friend code (the online Local Battle). The
        // process was made at boot
        0x29 => {
            set_result64(system, 0);
            "process.GetHandleInfo"
        }
        0x2A => {
            get_system_info(system);
            "process.GetSystemInfo"
        }
        0x2B => {
            let value: u64 = match system.cpu.regs[2] {
                // total memory the process has committed
                2 => system.memory.phys.region_used(system.kernel.memory_region) as u64,
                // what turns a linear heap address into a physical one
                20 => 0x2000_0000u32.wrapping_sub(system.kernel.linear_base) as u64,
                _ => 0,
            };
            set_result64(system, value);
            "process.GetProcessInfo"
        }
        // ConnectToPort(name): only "srv:" is ever asked for; its requests
        // (GetServiceHandle and the rest) are answered natively by the
        // overrides of nn::srv, so the port is a handle and nothing more
        0x2D => {
            let name = system.memory.read_cstring(system.cpu.regs[1], 12);
            log::debug!("svcConnectToPort(\"{name}\")");
            let object = system.kernel.objects.insert(KObject::ClientPort(name.clone()));
            let handle = system.kernel.handles.create(&mut system.kernel.objects, object, &name);
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = handle;
            "process.ConnectToPort"
        }
        0x38 => {
            let object = system.kernel.objects.insert(KObject::ResourceLimit);
            let handle = system.kernel.handles.create(&mut system.kernel.objects, object, "ResourceLimit");
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = handle;
            "process.GetResourceLimit"
        }
        0x39 => {
            resource_limit_values(system, false);
            "process.GetResourceLimitLimitValues"
        }
        0x3A => {
            resource_limit_values(system, true);
            "process.GetResourceLimitCurrentValues"
        }
        0x3C => {
            svc_break(system);
            "process.Break"
        }
        0x3D => {
            let ptr = system.cpu.regs[0];
            let len = system.cpu.regs[1].min(0x1000) as usize;
            let mut buf = vec![0u8; len];
            system.memory.read_bytes(ptr, &mut buf);
            let text = String::from_utf8_lossy(&buf);
            log::info!("[guest] {}", text.trim_end());
            system.debug_output.push_str(text.trim_end());
            system.debug_output.push('\n');
            system.cpu.regs[0] = 0;
            "process.OutputDebugString"
        }
        // cache maintenance: the host's caches are coherent
        0x54..=0x56 => {
            system.cpu.regs[0] = 0;
            "process.CacheMaintenance"
        }
        _ => return false,
    };
    stats::native(name);
    true
}

fn set_result64(system: &mut System, value: u64) {
    system.cpu.regs[0] = 0;
    system.cpu.regs[1] = value as u32;
    system.cpu.regs[2] = (value >> 32) as u32;
}

/// svcControlMemory, r0 = operation, r1 = addr0, r2 = addr1, r3 = size,
/// r4 = permission; the address in r1.
fn control_memory(system: &mut System) {
    let [operation, addr0, addr1, size, permission] = [0, 1, 2, 3, 4].map(|r| system.cpu.regs[r]);
    let op = operation & MEMOP_OP_MASK;
    let linear = operation & MEMOP_LINEAR_FLAG != 0;
    if size & PAGE_MASK != 0 {
        log::warn!("svcControlMemory with unaligned size 0x{size:X}");
        system.cpu.regs[0] = errors::MISALIGNED_SIZE.0;
        return;
    }
    let perm = Permission::from_bits_truncate(permission & 0x7);
    match op {
        MEMOP_ALLOC if linear => {
            let region = system.kernel.memory_region;
            let Some(block) = system.memory.phys.allocate(region, size) else {
                log::error!("out of linear memory allocating 0x{size:X}");
                system.cpu.regs[0] = errors::OUT_OF_MEMORY.0;
                return;
            };
            // the linear heap's virtual addresses are its physical ones
            // moved, which the GPU-facing code works out addresses with
            let vaddr = system.kernel.linear_base + (block.addr - FCRAM_PADDR);
            system.memory.map(vaddr, block.addr, block.size, perm | Permission::READ, MemoryState::Continuous);
            system.kernel.linear_top = system.kernel.linear_top.max(vaddr + block.size);
            log::debug!("linear alloc 0x{size:X} -> va 0x{vaddr:08X} pa 0x{:08X}", block.addr);
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = vaddr;
        }
        MEMOP_ALLOC => {
            let base = if addr0 == 0 { system.kernel.heap_top } else { addr0 };
            let region = system.kernel.memory_region;
            let Some(block) = system.memory.phys.allocate_top(region, size) else {
                log::error!("out of heap memory allocating 0x{size:X}");
                system.cpu.regs[0] = errors::OUT_OF_MEMORY.0;
                return;
            };
            system.memory.map(base, block.addr, block.size, perm | Permission::READ, MemoryState::Private);
            system.kernel.heap_top = system.kernel.heap_top.max(base + block.size);
            log::debug!("heap alloc 0x{size:X} at 0x{base:08X}");
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = base;
        }
        MEMOP_FREE => {
            // the pages behind the range go back, from where it starts in
            // each heap or linear mapping it covers
            let end = addr0 as u64 + size as u64;
            let freed: Vec<PhysicalBlock> = system
                .memory
                .mappings()
                .filter(|m| matches!(m.state, MemoryState::Private | MemoryState::Continuous))
                .filter(|m| (m.base as u64) < end && (addr0 as u64) < m.base as u64 + m.size as u64)
                .map(|m| {
                    let start = m.base.max(addr0);
                    let stop = (m.base as u64 + m.size as u64).min(end);
                    PhysicalBlock { addr: m.paddr + (start - m.base), size: (stop - start as u64) as u32 }
                })
                .collect();
            for block in freed {
                system.memory.phys.free(block);
            }
            system.memory.unmap(addr0, size);
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = addr0;
        }
        MEMOP_MAP => {
            // the pages backing addr1 also at addr0
            if let Some(mapping) = system.memory.mapping_at(addr1).copied() {
                let offset = addr1 - mapping.base;
                system.memory.map(addr0, mapping.paddr + offset, size, perm | Permission::RW, MemoryState::Aliased);
                system.cpu.regs[0] = 0;
            } else {
                log::warn!("svcControlMemory MAP of unmapped source 0x{addr1:08X}");
                system.cpu.regs[0] = errors::INVALID_ADDRESS.0;
            }
            system.cpu.regs[1] = addr0;
        }
        MEMOP_UNMAP => {
            system.memory.unmap(addr0, size);
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = addr0;
        }
        MEMOP_PROTECT | MEMOP_RESERVE => {
            // permissions are not enforced: accepted as they are
            system.cpu.regs[0] = 0;
            system.cpu.regs[1] = addr0;
        }
        _ => {
            log::warn!("svcControlMemory with unknown operation 0x{operation:X}");
            system.cpu.regs[0] = errors::INVALID_ENUM_VALUE.0;
        }
    }
}

/// svcCreateMemoryBlock, r0 = other permission, r1 = address, r2 = size,
/// r3 = own permission; the handle in r1.
fn create_memory_block(system: &mut System) {
    let (addr, size) = (system.cpu.regs[1], system.cpu.regs[2]);
    // address zero takes the pages from the BASE region, or from the
    // process's own region when its exheader says shared device memory
    let region = if system.kernel.shared_device_memory { system.kernel.memory_region } else { MemoryRegion::Base };
    let paddr = if addr == 0 {
        match system.memory.phys.allocate(region, size) {
            Some(block) => {
                system.memory.zero_physical(block.addr, block.size);
                block.addr
            }
            None => {
                system.cpu.regs[0] = errors::OUT_OF_MEMORY.0;
                return;
            }
        }
    } else {
        match system.memory.mapping_at(addr) {
            Some(mapping) => mapping.paddr + (addr - mapping.base),
            None => {
                system.cpu.regs[0] = errors::INVALID_ADDRESS.0;
                return;
            }
        }
    };
    let block = SharedMemory { name: format!("MemoryBlock@{addr:08X}"), address: addr, size, paddr, mapped_at: None };
    let object = system.kernel.objects.insert(KObject::SharedMemory(block));
    let handle = system.kernel.handles.create(&mut system.kernel.objects, object, "MemoryBlock");
    system.cpu.regs[0] = 0;
    system.cpu.regs[1] = handle;
}

/// svcMapMemoryBlock, r0 = handle, r1 = address, r2 = own permission,
/// r3 = other permission.
fn map_memory_block(system: &mut System) {
    let (handle, addr, permission) = (system.cpu.regs[0], system.cpu.regs[1], system.cpu.regs[2]);
    let block = system.kernel.resolve(handle).and_then(|object| match system.kernel.objects.get(object) {
        Some(KObject::SharedMemory(block)) => Some((object, block.paddr, block.size, block.address)),
        _ => None,
    });
    let Some((object, paddr, size, own)) = block else {
        log::warn!("svcMapMemoryBlock: handle 0x{handle:X} does not exist (called from 0x{:08X})", system.cpu.current_pc());
        system.cpu.regs[0] = errors::INVALID_HANDLE.0;
        return;
    };
    let addr = if addr == 0 && own != 0 {
        // a block kept somewhere already (the shared font) goes where it is
        own
    } else if addr == 0 {
        // the first place in the shared memory region the whole block fits
        let pages = size.div_ceil(PAGE_SIZE);
        let free = |at: u32| (0..pages).all(|page| system.memory.mapping_at(at + page * PAGE_SIZE).is_none());
        let mut candidate = SHARED_MEMORY_VADDR;
        while !free(candidate) {
            candidate += PAGE_SIZE;
        }
        candidate
    } else {
        addr
    };
    let perm = Permission::from_bits_truncate(permission & 0x7) | Permission::READ;
    system.memory.map(addr, paddr, size, perm, MemoryState::Shared);
    let name = match system.kernel.objects.get_mut(object) {
        Some(KObject::SharedMemory(block)) => {
            block.mapped_at = Some(addr);
            block.name.clone()
        }
        _ => String::new(),
    };
    // the native layers that publish a shared block write it where the
    // game put it
    match name.as_str() {
        "GSP" => system.services.gsp.shared_memory_address = addr,
        "HID" => {
            system.services.hid.shared_memory_address = addr;
            system.services.hid.shared_memory_paddr = paddr;
        }
        // the font's pointers are absolute: they mean something once the
        // game chose where the block lives
        "SharedFont" => crate::services::shared_font::relocate(&mut system.memory, addr, paddr),
        _ => {}
    }
    log::debug!("mapped {name} block handle 0x{handle:X} at 0x{addr:08X} (0x{size:X} bytes)");
    system.cpu.regs[0] = 0;
}

/// svcGetSystemInfo, r1 = type, r2 = parameter.
fn get_system_info(system: &mut System) {
    let (kind, param) = (system.cpu.regs[1], system.cpu.regs[2]);
    let value: u64 = match (kind, param) {
        // memory used in each FCRAM region
        (0, 0) => system.memory.phys.region_used(MemoryRegion::Application) as u64,
        (0, 1) => system.memory.phys.region_used(MemoryRegion::System) as u64,
        (0, 2) => system.memory.phys.region_used(MemoryRegion::Base) as u64,
        (2, 0) => system.kernel.live_thread_count() as u64,
        // kernel version
        (26, _) => 0,
        _ => {
            log::debug!("svcGetSystemInfo({kind}, {param}) is not implemented");
            0
        }
    };
    set_result64(system, value);
}

/// resource limit names, as svcGetResourceLimitLimitValues indexes them.
const RESOURCE_PRIORITY: u32 = 0;
const RESOURCE_COMMIT: u32 = 1;
const RESOURCE_THREAD: u32 = 2;
const RESOURCE_EVENT: u32 = 3;
const RESOURCE_MUTEX: u32 = 4;
const RESOURCE_SEMAPHORE: u32 = 5;
const RESOURCE_TIMER: u32 = 6;
const RESOURCE_SHARED_MEMORY: u32 = 7;
const RESOURCE_ADDRESS_ARBITER: u32 = 8;
const RESOURCE_CPU_TIME: u32 = 9;

/// svcGetResourceLimit(Limit|Current)Values, r0 = values out, r2 = names,
/// r3 = count.
fn resource_limit_values(system: &mut System, current: bool) {
    let (values_ptr, names_ptr, count) = (system.cpu.regs[0], system.cpu.regs[2], system.cpu.regs[3]);
    for i in 0..count {
        let name = system.memory.read32(names_ptr + i * 4);
        let value: i64 = match (name, current) {
            (RESOURCE_COMMIT, false) => system.memory.phys.region_size(system.kernel.memory_region) as i64,
            (RESOURCE_COMMIT, true) => system.memory.phys.region_used(system.kernel.memory_region) as i64,
            (RESOURCE_PRIORITY, _) => 0x18,
            (RESOURCE_THREAD, false) => 128,
            (RESOURCE_THREAD, true) => system.kernel.live_thread_count() as i64,
            (RESOURCE_EVENT, false) => 128,
            (RESOURCE_MUTEX, false) => 64,
            (RESOURCE_SEMAPHORE, false) => 32,
            (RESOURCE_TIMER, false) => 16,
            (RESOURCE_SHARED_MEMORY, false) => 32,
            (RESOURCE_ADDRESS_ARBITER, false) => 8,
            (RESOURCE_CPU_TIME, _) => 0,
            _ => 0,
        };
        system.memory.write32(values_ptr + i * 8, value as u32);
        system.memory.write32(values_ptr + i * 8 + 4, (value >> 32) as u32);
    }
    system.cpu.regs[0] = 0;
}

/// svcBreak: the game stopped itself (an assertion or a panic).
fn svc_break(system: &mut System) {
    let kind = match system.cpu.regs[0] {
        0 => "panic",
        1 => "assert",
        2 => "user",
        _ => "unknown",
    };
    log::error!("the game called svcBreak ({kind}) at 0x{:08X}", system.cpu.current_pc());
    log::error!("{:?}", system.cpu);
    log::error!("the code run last was:");
    let recent = system.recent_instructions();
    for &entry in recent.iter().rev().take(24).rev() {
        log::error!("  0x{:08X} {}", entry & !1, if entry & 1 != 0 { "T" } else { "A" });
    }
    system.broke = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// svcControlMemory with an operation, giving back r0 and r1.
    fn control(system: &mut System, operation: u32, addr0: u32, size: u32) -> (u32, u32) {
        system.cpu.regs[..5].copy_from_slice(&[operation, addr0, 0, size, 3]);
        assert!(svc(system, 0x01));
        (system.cpu.regs[0], system.cpu.regs[1])
    }

    /// the heap comes from the top of the region and the linear heap from
    /// its bottom, and a heap shrunk from its end gives back its end.
    #[test]
    fn the_linear_heap_starts_where_the_console_has_it() {
        let mut system = System::new(crate::Config::default());
        assert_eq!(control(&mut system, MEMOP_ALLOC, HEAP_VADDR, 0x4000), (0, HEAP_VADDR));
        let linear = control(&mut system, MEMOP_ALLOC | MEMOP_LINEAR_FLAG, 0, 0x2000);
        assert_eq!(linear, (0, system.kernel.linear_base));
        let heap = system.memory.mapping_at(HEAP_VADDR).unwrap().paddr;
        assert!(heap > FCRAM_PADDR + 0x2000);
        assert_eq!(control(&mut system, MEMOP_FREE, HEAP_VADDR + 0x2000, 0x2000).0, 0);
        let kept = system.memory.mapping_at(HEAP_VADDR).unwrap();
        assert_eq!((kept.paddr, kept.size), (heap, 0x2000));
        assert!(system.memory.mapping_at(HEAP_VADDR + 0x2000).is_none());
        assert_eq!(control(&mut system, MEMOP_ALLOC, HEAP_VADDR + 0x2000, 0x2000).0, 0);
        assert_eq!(system.memory.mapping_at(HEAP_VADDR + 0x2000).unwrap().paddr, heap + 0x2000);
    }

    /// two reads of the clock in a row are as far apart as on the console.
    #[test]
    fn reading_the_clock_takes_time() {
        let mut system = System::new(crate::Config::default());
        let mut read = || {
            assert!(svc(&mut system, 0x28));
            system.cpu.regs[0] as u64 | (system.cpu.regs[1] as u64) << 32
        };
        let first = read();
        assert_eq!(read() - first, super::super::svc::SYSTEM_TICK_COST);
    }
}
