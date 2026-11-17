//! os2: the GPU service calls (nn::gxlow::CTR::Gpu) without IPC.
//!
//! nn::gxlow talks to the GSP module through `nn::gxlow::CTR::Gpu`, one IPC
//! request per call: TriggerCmdReqQueue (run the GX commands queued in the
//! shared memory ring, about 14 a frame), FlushDataCache / StoreDataCache
//! (cache maintenance, 2 a frame), WriteHWRegs / WriteHWRegsWithMask
//! (display registers), SetLcdForceBlack, AcquireRight and, once at start,
//! RegisterInterruptRelayQueue (the event the GPU interrupt receiver thread
//! waits on, and the shared memory). overrides/os2_native.c sends each one
//! here instead. What each does is what the GSP service did, without the
//! command buffer, the session or the service dispatch: the GX ring is
//! processed by the same command processing (services::gsp, which drives
//! the remaining emulated GPU work: display transfers and fills, the unit
//! icons' capture), and its interrupts reach the GPU interrupt receiver as
//! native wake-ups (pcos::sched::signal_event). Each call keeps the
//! request's console timing (bridge::console_timing).

use zakuro_cpu::Bus;

use super::bridge::Call;
use super::stats;
use zakuro_common::memory_map::{FCRAM_PADDR, PAGE_SIZE};

use crate::kernel::object::{KObject, SharedMemory};
use crate::System;

/// call numbers, the same as in overrides/os2_native.c.
pub mod op {
    pub const TRIGGER_CMD_REQ_QUEUE: u32 = 0x600;
    pub const FLUSH_DATA_CACHE: u32 = 0x601;
    pub const STORE_DATA_CACHE: u32 = 0x602;
    pub const WRITE_HW_REGS: u32 = 0x603;
    pub const WRITE_HW_REGS_WITH_MASK: u32 = 0x604;
    pub const SET_LCD_FORCE_BLACK: u32 = 0x605;
    pub const ACQUIRE_RIGHT: u32 = 0x606;
    pub const RELEASE_RIGHT: u32 = 0x607;
    pub const REGISTER_INTERRUPT_RELAY_QUEUE: u32 = 0x608;
    pub const UNREGISTER_INTERRUPT_RELAY_QUEUE: u32 = 0x609;
}

/// runs a Gpu call natively; false to leave it to IPC. Every Gpu method
/// takes the session (`this`) first.
pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    let name = match number {
        op::TRIGGER_CMD_REQ_QUEUE => {
            // the GPU's work reads guest memory itself, outside the code's
            // view of it, as it did from the service
            system.memory.clear_gpu_sync();
            trigger_cmd_req_queue(system);
            system.sync_gpu_for_cpu();
            call.ret(0);
            "gsp.TriggerCmdReqQueue"
        }
        // the host's caches are coherent
        op::FLUSH_DATA_CACHE => {
            call.ret(0);
            "gsp.FlushDataCache"
        }
        op::STORE_DATA_CACHE => {
            call.ret(0);
            "gsp.StoreDataCache"
        }
        // WriteHWRegs(this, offset, data, size)
        op::WRITE_HW_REGS => {
            let (offset, data, size) = (call.arg(system, 1), call.arg(system, 2), call.arg(system, 3));
            system.memory.clear_gpu_sync();
            write_hw_regs(system, offset, size, data, None);
            system.sync_gpu_for_cpu();
            call.ret(0);
            "gsp.WriteHWRegs"
        }
        // WriteHWRegsWithMask(this, offset, data, mask, size)
        op::WRITE_HW_REGS_WITH_MASK => {
            let (offset, data, mask, size) = (call.arg(system, 1), call.arg(system, 2), call.arg(system, 3), call.arg(system, 4));
            system.memory.clear_gpu_sync();
            write_hw_regs(system, offset, size, data, Some(mask));
            system.sync_gpu_for_cpu();
            call.ret(0);
            "gsp.WriteHWRegsWithMask"
        }
        // SetLcdForceBlack(this, bool): a byte, whatever the rest holds
        op::SET_LCD_FORCE_BLACK => {
            system.lcd_force_black = call.arg(system, 1) & 0xFF != 0;
            call.ret(0);
            "gsp.SetLcdForceBlack"
        }
        op::ACQUIRE_RIGHT => {
            system.services.gsp.has_right = true;
            call.ret(0);
            "gsp.AcquireRight"
        }
        op::RELEASE_RIGHT => {
            system.services.gsp.has_right = false;
            call.ret(0);
            "gsp.ReleaseRight"
        }
        // RegisterInterruptRelayQueue(this, event, flags, Handle* shared
        // memory, int* thread index): 0x2A07, success that also says the GPU
        // was set up already, so the SDK skips its first-time setup
        op::REGISTER_INTERRUPT_RELAY_QUEUE => {
            let (event, memory_out, index_out) = (call.arg(system, 1), call.arg(system, 3), call.arg(system, 4));
            let (handle, index) = register_interrupt_relay_queue(system, event);
            system.memory.write32(memory_out, handle);
            system.memory.write32(index_out, index);
            call.ret(0x2A07);
            "gsp.RegisterInterruptRelayQueue"
        }
        op::UNREGISTER_INTERRUPT_RELAY_QUEUE => {
            unregister_interrupt_relay_queue(system);
            call.ret(0);
            "gsp.UnregisterInterruptRelayQueue"
        }
        _ => return false,
    };
    stats::native(name);
    true
}

// -- native-final: the GSP's shared memory, interrupts, framebuffers and GX ring --
//
// The GPU service's state as the game sees it (moved here from services::gsp,
// which keeps only the IPC request handler of development builds).

/// size of the GSP shared memory block.
pub const SHARED_MEMORY_SIZE: u32 = 0x1000;
/// start of the GX command ring within it.
const COMMAND_BUFFER_BASE: u32 = 0x800;
const COMMAND_BUFFER_STRIDE: u32 = 0x200;
const INTERRUPT_QUEUE_STRIDE: u32 = 0x40;

/// start of the FrameBufferInfo table, one 0x40-byte record per screen per
/// client, [client][screen].
const FRAMEBUFFER_INFO_BASE: u32 = 0x200;
const FRAMEBUFFER_INFO_CLIENT_STRIDE: u32 = 0x80;
const FRAMEBUFFER_INFO_SCREEN_STRIDE: u32 = 0x40;
/// offset of framebuf_info[0] within a screen's record, framebuf_info[1]
/// follows immediately at + 0x1C.
const FRAMEBUFFER_ENTRY_BASE: u32 = 0x04;
const FRAMEBUFFER_ENTRY_STRIDE: u32 = 0x1C;

/// interrupts GSP relays to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum InterruptId {
    Psc0 = 0,
    Psc1 = 1,
    /// top screen vertical blank.
    Pdc0 = 2,
    /// bottom screen vertical blank.
    Pdc1 = 3,
    /// display transfer / texture copy finished.
    Ppf = 4,
    /// command list finished.
    P3d = 5,
    Dma = 6,
}

/// one entry from the GX command ring.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct GxCommand {
    pub id: u8,
    pub data: [u32; 7],
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct GspState {
    pub shared_memory_handle: Option<u32>,
    pub shared_memory_address: u32,
    /// event the client waits on for relayed interrupts.
    pub interrupt_event: Option<u32>,
    pub interrupt_event_object: Option<crate::kernel::object::ObjectId>,
    /// which slot in the shared memory this client owns.
    pub thread_index: u32,
    /// true once a client has taken GPU rights.
    pub has_right: bool,
    /// commands pulled off the ring that the GPU has not run yet.
    pub pending: Vec<GxCommand>,
    /// the picture each screen has waiting in shared memory for the next
    /// refresh, as the first look since the last one found it.
    pub waiting: [Option<(u32, u32)>; 2],
    /// the ring waits for the next refresh, see outdrawn.
    pub held: bool,
}

/// applies any buffer swap a game has queued in the FrameBufferInfo table for
/// either screen, the way real GSP does once per vertical blank.
pub fn apply_framebuffer_updates(system: &mut System) {
    let base = system.services.gsp.shared_memory_address;
    if base == 0 {
        return;
    }
    let client = system.services.gsp.thread_index * FRAMEBUFFER_INFO_CLIENT_STRIDE;

    for screen in 0..2u32 {
        let record = base + FRAMEBUFFER_INFO_BASE + client + screen * FRAMEBUFFER_INFO_SCREEN_STRIDE;
        let header = system.memory.read32(record);
        let index = header & 0xFF;
        let is_dirty = (header >> 8) & 0xFF;
        if is_dirty == 0 {
            continue;
        }

        let entry = record + FRAMEBUFFER_ENTRY_BASE + index * FRAMEBUFFER_ENTRY_STRIDE;
        let left = system.memory.read32(entry + 0x04);
        let left = virtual_to_physical(system, left);
        let right = system.memory.read32(entry + 0x08);
        let right = virtual_to_physical(system, right);
        let stride = system.memory.read32(entry + 0x0C);
        let format = system.memory.read32(entry + 0x10);
        system.set_framebuffer(screen, index, left, right, stride, format);

        // real GSP clears the flag once it has picked the update up, so a
        // title that polls it to know its previous frame was consumed keeps
        // moving instead of assuming GSP is stuck.
        system.memory.write32(record, index);
    }
}

/// whether the title drew past what the screens can show, a picture over
/// one still waiting for a refresh, so the ring waits for the next one.
/// the console's GPU takes its time, which paces a title that never waits
/// for a refresh, and ours finishes at once, Monster Hunter 3 Ultimate drew
/// twenty pictures for each refresh that no one saw. a title that waits for
/// a refresh sets one picture for each and is never held.
pub(crate) fn outdrawn(system: &mut System) -> bool {
    let base = system.services.gsp.shared_memory_address;
    if base == 0 || system.services.gsp.held {
        return system.services.gsp.held;
    }
    let client = system.services.gsp.thread_index * FRAMEBUFFER_INFO_CLIENT_STRIDE;
    for screen in 0..2 {
        let record = base + FRAMEBUFFER_INFO_BASE + client + screen as u32 * FRAMEBUFFER_INFO_SCREEN_STRIDE;
        let header = system.memory.read32(record);
        if (header >> 8) & 0xFF == 0 {
            continue;
        }
        let index = header & 0xFF;
        let left = system.memory.read32(record + FRAMEBUFFER_ENTRY_BASE + index * FRAMEBUFFER_ENTRY_STRIDE + 0x04);
        match system.services.gsp.waiting[screen] {
            None => system.services.gsp.waiting[screen] = Some((index, left)),
            Some(waiting) if waiting != (index, left) => system.services.gsp.held = true,
            Some(_) => {}
        }
    }
    system.services.gsp.held
}

/// the screens refresh, taking the pictures set for them, and a ring held
/// for it runs.
pub fn refresh(system: &mut System) {
    apply_framebuffer_updates(system);
    system.services.gsp.waiting = [None; 2];
    if std::mem::take(&mut system.services.gsp.held) {
        process_command_queue(system);
    }
}

/// decodes the fill width from a memory-fill control word.
pub(crate) fn fill_width(control: u32) -> u32 {
    match control & 0x300 {
        0x000 => 2, // 16 bits per pixel
        0x100 => 3, // 24 bits
        _ => 4,     // 32 bits
    }
}

/// creates the shared memory block on first use and maps it for the client.
pub(crate) fn ensure_shared_memory(system: &mut System) -> u32 {
    if let Some(handle) = system.services.gsp.shared_memory_handle {
        return handle;
    }

    let block = system
        .memory
        .phys
        .allocate(crate::memory::MemoryRegion::Base, SHARED_MEMORY_SIZE)
        .expect("GSP shared memory");

    let object = system
        .kernel
        .objects
        .insert(KObject::SharedMemory(SharedMemory {
            name: "GSP".into(),
            address: 0,
            size: SHARED_MEMORY_SIZE,
            paddr: block.addr,
            mapped_at: None,
        }));
    let handle = system
        .kernel
        .handles
        .create(&mut system.kernel.objects, object, "GSP shared memory");
    system.services.gsp.shared_memory_handle = Some(handle);
    handle
}

/// address of this client's interrupt queue inside the shared block.
pub(crate) fn interrupt_queue_address(system: &System) -> Option<u32> {
    let base = system.services.gsp.shared_memory_address;
    (base != 0).then(|| base + system.services.gsp.thread_index * INTERRUPT_QUEUE_STRIDE)
}

/// posts an interrupt to the client and signals its event.
pub fn signal_interrupt(system: &mut System, interrupt: InterruptId) {
    let Some(queue) = interrupt_queue_address(system) else {
        return;
    };

    let index = system.memory.read8(queue) as u32;
    let count = system.memory.read8(queue + 1) as u32;

    if count >= 0x34 {
        // the queue is full, hardware sets the error byte and drops it.
        system.memory.write8(queue + 2, 1);
        return;
    }

    let slot = (index + count) % 0x34;
    system.memory.write8(queue + 0xC + slot, interrupt as u8);
    system.memory.write8(queue + 1, (count + 1) as u8);

    if let Some(object) = system.services.gsp.interrupt_event_object {
        crate::pcos::sched::signal_event(system, object); // os2: a native wake-up
    }
}

/// os2: TriggerCmdReqQueue, run whatever the client queued in shared
/// memory (also called natively, pcos::gsp).
pub(crate) fn trigger_cmd_req_queue(system: &mut System) {
    if !outdrawn(system) {
        process_command_queue(system);
    }
}

/// os2: RegisterInterruptRelayQueue(event): the shared memory handle and
/// the client's slot (also called natively, pcos::gsp).
pub(crate) fn register_interrupt_relay_queue(system: &mut System, event: u32) -> (u32, u32) {
    system.services.gsp.interrupt_event = Some(event);
    // kept while registered, whatever the title does with its handle
    let object = system.kernel.resolve(event);
    if let Some(object) = object {
        system.kernel.objects.add_ref(object);
    }
    if let Some(old) = std::mem::replace(&mut system.services.gsp.interrupt_event_object, object) {
        system.kernel.objects.release(old);
    }
    let handle = ensure_shared_memory(system);
    let index = system.services.gsp.thread_index;
    log::debug!("gsp: registered interrupt relay queue, slot {index}");
    (handle, index)
}

/// os2: UnregisterInterruptRelayQueue (also called natively, pcos::gsp).
pub(crate) fn unregister_interrupt_relay_queue(system: &mut System) {
    system.services.gsp.interrupt_event = None;
    if let Some(old) = system.services.gsp.interrupt_event_object.take() {
        system.kernel.objects.release(old);
    }
}

pub(crate) fn write_hw_regs(system: &mut System, offset: u32, size: u32, source: u32, mask: Option<u32>) {
    // GSP only lets a client touch the GPU register window.
    if offset >= 0x420000 || size > 0x80 {
        log::warn!("gsp WriteHWRegs out of range: offset 0x{offset:X} size 0x{size:X}");
        return;
    }
    for i in 0..size / 4 {
        let value = system.memory.read32(source + i * 4);
        let register = offset + i * 4;
        match mask {
            Some(mask_ptr) => {
                let mask = system.memory.read32(mask_ptr + i * 4);
                let old = system.gpu_read_register(register);
                system.gpu_write_register(register, (old & !mask) | (value & mask));
            }
            None => system.gpu_write_register(register, value),
        }
    }
}

/// pulls every queued GX command off the ring and runs it.
pub(crate) fn process_command_queue(system: &mut System) {
    let base = system.services.gsp.shared_memory_address;
    if base == 0 {
        return;
    }
    let ring = base + COMMAND_BUFFER_BASE
        + system.services.gsp.thread_index * COMMAND_BUFFER_STRIDE;

    loop {
        let control = system.memory.read32(ring);
        let index = control & 0xFF;
        let count = (control >> 8) & 0xFF;
        if count == 0 {
            break;
        }

        let entry = ring + 0x20 + (index % 15) * 0x20;
        let header = system.memory.read32(entry);
        let mut data = [0u32; 7];
        for (i, word) in data.iter_mut().enumerate() {
            *word = system.memory.read32(entry + 4 + i as u32 * 4);
        }

        let command = GxCommand {
            id: (header & 0xFF) as u8,
            data,
        };

        // consume it before running, so a command that re-enters GSP sees a
        // consistent ring.
        let next_index = (index + 1) % 15;
        system
            .memory
            .write32(ring, next_index | ((count - 1) << 8));

        execute_command(system, command);
    }
}

pub(crate) fn execute_command(system: &mut System, command: GxCommand) {
    log::trace!(
        "gx command 0x{:02X}: {:08X?}",
        command.id,
        &command.data[..5]
    );
    match command.id {
        // RequestDma(source, destination, size)
        0x00 => {
            let (src, dst, size) = (command.data[0], command.data[1], command.data[2]);
            system.sync_gpu(src, size);
            system.sync_gpu(dst, size);
            let mut buf = vec![0u8; size as usize];
            system.memory.read_bytes(src, &mut buf);
            system.memory.write_bytes(dst, &buf);
            signal_interrupt(system, InterruptId::Dma);
        }
        // ProcessCommandList(address, size, ...)
        0x01 => {
            let address = virtual_to_physical(system, command.data[0] & !7);
            let size = command.data[1] & !3;
            system.submit_command_list(address, size);
            signal_interrupt(system, InterruptId::P3d);
        }
        // MemoryFill(start0, value0, end0, start1, value1, end1, control)
        0x02 => {
            for bank in 0..2 {
                let start = command.data[bank * 3];
                let value = command.data[bank * 3 + 1];
                let end = command.data[bank * 3 + 2];
                if start == 0 || end <= start {
                    continue;
                }
                let width = fill_width(command.data[6] >> (bank as u32 * 16));
                let physical = virtual_to_physical(system, start);
                system.memory_fill(physical, physical + (end - start), value, width);
                signal_interrupt(
                    system,
                    if bank == 0 {
                        InterruptId::Psc0
                    } else {
                        InterruptId::Psc1
                    },
                );
            }
        }
        // DisplayTransfer(input, output, input dim, output dim, flags)
        0x03 => {
            system.display_transfer(
                virtual_to_physical(system, command.data[0]),
                virtual_to_physical(system, command.data[1]),
                command.data[2],
                command.data[3],
                command.data[4],
            );
            signal_interrupt(system, InterruptId::Ppf);
        }
        // TextureCopy(input, output, size, input gap, output gap, flags)
        0x04 => {
            system.texture_copy(
                virtual_to_physical(system, command.data[0]),
                virtual_to_physical(system, command.data[1]),
                command.data[2],
                command.data[3],
                command.data[4],
            );
            signal_interrupt(system, InterruptId::Ppf);
        }
        // CacheFlush
        0x05 => {}
        other => log::warn!("unknown GX command 0x{other:02X}"),
    }
}

/// converts an address a title hands the GSP, a virtual one in VRAM or its
/// linear heap, to the physical one the GPU takes, as the GSP does. the
/// linear heap at 0x30000000 is above where FCRAM starts, so the GPU can't
/// tell one from the other itself. anything else goes as it is.
pub fn virtual_to_physical(system: &System, vaddr: u32) -> u32 {
    use zakuro_common::memory_map::*;
    let linear = system.kernel.linear_base;
    let size = if linear == LINEAR_HEAP_VADDR_NEW3DS { LINEAR_HEAP_SIZE_NEW3DS } else { LINEAR_HEAP_SIZE_OLD3DS };
    if (VRAM_VADDR..VRAM_VADDR + VRAM_SIZE).contains(&vaddr) {
        VRAM_PADDR + (vaddr - VRAM_VADDR)
    } else if vaddr >= linear && vaddr - linear < size {
        FCRAM_PADDR + (vaddr - linear)
    } else {
        vaddr
    }
}

/// converts a physical address in a GX command to the linear-heap virtual
/// address it corresponds to, which is where our memory actually is.
pub fn physical_to_virtual(system: &System, paddr: u32) -> u32 {
    if paddr >= FCRAM_PADDR {
        system.kernel.linear_base + (paddr - FCRAM_PADDR)
    } else if (0x1800_0000..0x1860_0000).contains(&paddr) {
        zakuro_common::memory_map::VRAM_VADDR + (paddr - 0x1800_0000)
    } else {
        paddr
    }
}

/// rounds a size up to whole pages, used when mapping the shared block.
pub const fn page_align(size: u32) -> u32 {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryState, Permission};
    use crate::{Config, System};

    /// maps the GSP shared memory block and points shared_memory_address
    /// at it, the way svcMapMemoryBlock does for a real title, so a test
    /// can write into the FrameBufferInfo table the way a game would.
    fn system_with_mapped_gsp_shm() -> (System, u32) {
        let mut system = System::new(Config::default());
        ensure_shared_memory(&mut system);
        let handle = system.services.gsp.shared_memory_handle.unwrap();
        let object = system.kernel.resolve(handle).unwrap();
        let paddr = match system.kernel.objects.get(object) {
            Some(KObject::SharedMemory(block)) => block.paddr,
            _ => panic!("expected the GSP block to exist"),
        };
        let vaddr = 0x1000_0000;
        system.memory.map(
            vaddr,
            paddr,
            SHARED_MEMORY_SIZE,
            Permission::READ | Permission::WRITE,
            MemoryState::Shared,
        );
        system.services.gsp.shared_memory_address = vaddr;
        (system, vaddr)
    }

    /// SetLcdForceBlack's flag is a u8. OoT3D sends false in a word whose top
    /// bytes still hold the result code of the reply before, after making
    /// its save on a first boot, and taking the whole word as the flag left
    /// the screen black for good.
    #[test]
    fn force_black_reads_only_the_low_byte() {
        let (mut system, vaddr) = system_with_mapped_gsp_shm();
        let buffer = CommandBuffer::new(vaddr);
        for (word, black) in [(1, true), (0xC8A0_4500, false), (0xC8A0_4501, true), (0, false)] {
            let header = Header::new(command::SET_LCD_FORCE_BLACK, 1, 0);
            buffer.set(&mut system.memory, 0, header.0);
            buffer.set(&mut system.memory, 1, word);
            handle(&mut system, &buffer, header);
            assert_eq!(system.lcd_force_black, black, "word 0x{word:08X}");
        }
    }

    /// this is the exact layout recovered by scanning a running Pokémon Alpha
    /// Sapphire's GSP shared memory for the physical addresses a real
    /// display transfer had just written into, a title toggles is_dirty
    /// and index in the FrameBufferInfo header instead of calling
    /// SetBufferSwap every frame, and a vblank that does not pick this up
    /// leaves the screen showing whatever address was configured at launch
    /// forever, no matter how correctly the rest of the GPU pipeline renders.
    #[test]
    fn picks_up_a_swap_queued_directly_in_shared_memory() {
        let (mut system, vaddr) = system_with_mapped_gsp_shm();
        let record = vaddr + FRAMEBUFFER_INFO_BASE; // screen 0, client 0
        system.memory.write32(record, 0x0000_0101); // index 1, is_dirty 1
        let entry = record + FRAMEBUFFER_ENTRY_BASE + FRAMEBUFFER_ENTRY_STRIDE; // buf[1]
        system.memory.write32(entry + 0x04, 0x1F30_0000); // address_left
        system.memory.write32(entry + 0x0C, 720); // stride
        system.memory.write32(entry + 0x10, 0x341); // format

        apply_framebuffer_updates(&mut system);

        // the GPU gets the physical address of the VRAM the title named
        assert_eq!(system.gpu.framebuffers[0].address_left(), 0x1830_0000);
        assert_eq!(system.gpu.framebuffers[0].stride, 720);
        assert_eq!(
            system.memory.read32(record) & 0xFF00,
            0,
            "is_dirty should be cleared once the swap is applied"
        );
    }

    /// a title that never waits for a refresh sets a picture over one the
    /// screens have not shown yet, and its commands wait in the ring for
    /// the next refresh, as with a GPU that takes its time. one that sets a
    /// picture for each refresh never waits.
    #[test]
    fn a_picture_over_one_not_shown_holds_the_ring_until_the_refresh() {
        let (mut system, vaddr) = system_with_mapped_gsp_shm();
        let record = vaddr + FRAMEBUFFER_INFO_BASE;
        let ring = vaddr + COMMAND_BUFFER_BASE;
        let picture = |system: &mut System, index: u32, left: u32| {
            system.memory.write32(record + FRAMEBUFFER_ENTRY_BASE + index * FRAMEBUFFER_ENTRY_STRIDE + 0x04, left);
            system.memory.write32(record, 0x100 | index);
        };
        // queues a cache flush, which does nothing, and asks for the ring to
        // run, what it leaves in it waits
        let trigger = |system: &mut System| {
            let control = system.memory.read32(ring);
            let (index, count) = (control & 0xFF, (control >> 8) & 0xFF);
            system.memory.write32(ring + 0x20 + (index + count) % 15 * 0x20, 0x05);
            system.memory.write32(ring, index | (count + 1) << 8);
            let buffer = CommandBuffer::new(vaddr + 0x100);
            let header = Header::new(command::TRIGGER_CMD_REQ_QUEUE, 0, 0);
            buffer.set(&mut system.memory, 0, header.0);
            handle(system, &buffer, header);
            (system.memory.read32(ring) >> 8) & 0xFF
        };

        for (index, left) in [(0, 0x1F30_0000), (1, 0x1F35_DC00), (0, 0x1F3B_B800)] {
            picture(&mut system, index, left);
            assert_eq!(trigger(&mut system), 0, "one picture for each refresh runs at once");
            refresh(&mut system);
        }
        picture(&mut system, 1, 0x1F30_0000);
        assert_eq!(trigger(&mut system), 0);
        picture(&mut system, 0, 0x1F35_DC00);
        assert_eq!(trigger(&mut system), 1, "a second picture before the refresh holds the ring");
        assert_eq!(trigger(&mut system), 2);
        refresh(&mut system);
        assert_eq!((system.memory.read32(ring) >> 8) & 0xFF, 0, "the refresh runs what waited");
        assert_eq!(system.gpu.framebuffers[0].address_left(), 0x1835_DC00, "showing the newest picture");
        picture(&mut system, 1, 0x1F3B_B800);
        assert_eq!(trigger(&mut system), 0);
    }

    /// a record with is_dirty clear (the steady state right after a swap
    /// was applied) must not be re-applied every frame, that would fight a
    /// title that legitimately wants to hold the same buffer for two frames.
    #[test]
    fn leaves_a_clean_record_alone() {
        let (mut system, vaddr) = system_with_mapped_gsp_shm();
        system.set_framebuffer(0, 0, 0x1234_0000, 0, 720, 0x341);

        apply_framebuffer_updates(&mut system);

        assert_eq!(system.gpu.framebuffers[0].address_left(), 0x1234_0000);
        let _ = vaddr;
    }
}

/// native-final: the refresh's VBlank interrupts, top screen then bottom.
pub fn vblank(system: &mut System) {
    signal_interrupt(system, InterruptId::Pdc0);
    signal_interrupt(system, InterruptId::Pdc1);
}
