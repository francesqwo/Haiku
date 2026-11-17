//! gsp::Gpu, the GPU service: the IPC request handler of development builds.
//! native-final: the shared memory, interrupt relay, framebuffer latch and
//! GX ring are native (pcos::gsp); this handler drives them for a game that
//! sends requests, which Fates does not.

use zakuro_cpu::Bus;

use crate::kernel::ipc::{CommandBuffer, Descriptor, Header};
use crate::System;

pub use crate::pcos::gsp::*;

/// command ids, straight from 3dbrew's GSP::GPU table.
mod command {
    pub const WRITE_HW_REGS: u16 = 0x0001;
    pub const WRITE_HW_REGS_WITH_MASK: u16 = 0x0002;
    pub const WRITE_HW_REG_REPEAT: u16 = 0x0003;
    pub const READ_HW_REGS: u16 = 0x0004;
    pub const SET_BUFFER_SWAP: u16 = 0x0005;
    pub const SET_COMMAND_LIST: u16 = 0x0006;
    pub const REQUEST_DMA: u16 = 0x0007;
    pub const FLUSH_DATA_CACHE: u16 = 0x0008;
    pub const INVALIDATE_DATA_CACHE: u16 = 0x0009;
    pub const REGISTER_INTERRUPT_EVENTS: u16 = 0x000A;
    pub const SET_LCD_FORCE_BLACK: u16 = 0x000B;
    pub const TRIGGER_CMD_REQ_QUEUE: u16 = 0x000C;
    pub const SET_DISPLAY_TRANSFER: u16 = 0x000D;
    pub const SET_TEXTURE_COPY: u16 = 0x000E;
    pub const SET_MEMORY_FILL: u16 = 0x000F;
    pub const SET_AXI_CONFIG_QOS_MODE: u16 = 0x0010;
    pub const SET_PERF_LOG_MODE: u16 = 0x0011;
    pub const GET_PERF_LOG: u16 = 0x0012;
    pub const REGISTER_INTERRUPT_RELAY_QUEUE: u16 = 0x0013;
    pub const UNREGISTER_INTERRUPT_RELAY_QUEUE: u16 = 0x0014;
    pub const TRY_ACQUIRE_RIGHT: u16 = 0x0015;
    pub const ACQUIRE_RIGHT: u16 = 0x0016;
    pub const RELEASE_RIGHT: u16 = 0x0017;
    pub const IMPORT_DISPLAY_CAPTURE_INFO: u16 = 0x0018;
    pub const SAVE_VRAM_SYS_AREA: u16 = 0x0019;
    pub const RESTORE_VRAM_SYS_AREA: u16 = 0x001A;
    pub const RESET_GPU_CORE: u16 = 0x001B;
    pub const SET_LED_FORCE_OFF: u16 = 0x001C;
    pub const SET_INTERNAL_PRIORITIES: u16 = 0x001E;
    pub const STORE_DATA_CACHE: u16 = 0x001F;
}

#[cfg(feature = "emulation")] // native-final: the IPC request handler, development builds only
pub fn handle(system: &mut System, buffer: &CommandBuffer, header: Header) -> bool {
    use command::*;
    let id = header.command_id();
    match id {
        // WriteHWRegs(offset, size, <static buffer>)
        WRITE_HW_REGS => {
            let offset = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 2);
            let source = buffer.get(&mut system.memory, 4);
            write_hw_regs(system, offset, size, source, None);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // WriteHWRegsWithMask(offset, size, <buffer>, <mask buffer>)
        WRITE_HW_REGS_WITH_MASK => {
            let offset = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 2);
            let source = buffer.get(&mut system.memory, 4);
            let mask = buffer.get(&mut system.memory, 6);
            write_hw_regs(system, offset, size, source, Some(mask));
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // WriteHWRegRepeat(offset, size, <buffer>), writes the same register
        // repeatedly, which is how command data is fed to the GPU.
        WRITE_HW_REG_REPEAT => {
            let offset = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 2);
            let source = buffer.get(&mut system.memory, 4);
            for i in 0..size / 4 {
                let value = system.memory.read32(source + i * 4);
                system.gpu_write_register(offset, value);
            }
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // ReadHWRegs(offset, size)
        READ_HW_REGS => {
            let offset = buffer.get(&mut system.memory, 1);
            let size = buffer.get(&mut system.memory, 2);
            let dest = buffer.get(&mut system.memory, 4);
            for i in 0..size / 4 {
                let value = system.gpu_read_register(offset + i * 4);
                system.memory.write32(dest + i * 4, value);
            }
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // SetBufferSwap(screen, FrameBufferInfo)
        SET_BUFFER_SWAP => {
            let screen = buffer.get(&mut system.memory, 1);
            let active = buffer.get(&mut system.memory, 2);
            let left = buffer.get(&mut system.memory, 3);
            let right = buffer.get(&mut system.memory, 4);
            let stride = buffer.get(&mut system.memory, 5);
            let format = buffer.get(&mut system.memory, 6);
            log::debug!(
                "gsp: SetBufferSwap screen {screen} active {active} left 0x{left:08X} right \
                 0x{right:08X} stride {stride} format 0x{format:X}"
            );
            let (left, right) = (virtual_to_physical(system, left), virtual_to_physical(system, right));
            system.set_framebuffer(screen, active, left, right, stride, format);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // SetCommandList(address, size, ...), the immediate form of the GX
        // command with the same name.
        SET_COMMAND_LIST => {
            let address = buffer.get(&mut system.memory, 1) & !7;
            let address = virtual_to_physical(system, address);
            let size = buffer.get(&mut system.memory, 2) & !3;
            system.submit_command_list(address, size);
            signal_interrupt(system, InterruptId::P3d);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // RequestDma(source, destination, size)
        REQUEST_DMA => {
            let source = buffer.get(&mut system.memory, 1);
            let dest = buffer.get(&mut system.memory, 2);
            let size = buffer.get(&mut system.memory, 3);
            system.sync_gpu(source, size);
            system.sync_gpu(dest, size);
            let mut data = vec![0u8; size as usize];
            system.memory.read_bytes(source, &mut data);
            system.memory.write_bytes(dest, &data);
            signal_interrupt(system, InterruptId::Dma);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // the cache maintenance commands, memory is coherent for us.
        FLUSH_DATA_CACHE | INVALIDATE_DATA_CACHE | STORE_DATA_CACHE => {
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // RegisterInterruptEvents
        REGISTER_INTERRUPT_EVENTS => {
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        SET_LCD_FORCE_BLACK => {
            // a u8, the rest of the word is whatever was there before, OoT3D
            // leaves a result code's top bytes in it after making its save
            system.lcd_force_black = buffer.get(&mut system.memory, 1) & 0xFF != 0;
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // TriggerCmdReqQueue, run whatever the client queued in shared memory.
        TRIGGER_CMD_REQ_QUEUE => {
            trigger_cmd_req_queue(system);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // SetDisplayTransfer(input, output, input dim, output dim, flags)
        SET_DISPLAY_TRANSFER => {
            let input = buffer.get(&mut system.memory, 1);
            let input = virtual_to_physical(system, input);
            let output = buffer.get(&mut system.memory, 2);
            let output = virtual_to_physical(system, output);
            let input_dimensions = buffer.get(&mut system.memory, 3);
            let output_dimensions = buffer.get(&mut system.memory, 4);
            let flags = buffer.get(&mut system.memory, 5);
            system.display_transfer(input, output, input_dimensions, output_dimensions, flags);
            signal_interrupt(system, InterruptId::Ppf);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // SetTextureCopy(input, output, size, input gap, output gap, flags)
        SET_TEXTURE_COPY => {
            let input = buffer.get(&mut system.memory, 1);
            let input = virtual_to_physical(system, input);
            let output = buffer.get(&mut system.memory, 2);
            let output = virtual_to_physical(system, output);
            let size = buffer.get(&mut system.memory, 3);
            let input_gap = buffer.get(&mut system.memory, 4);
            let output_gap = buffer.get(&mut system.memory, 5);
            system.texture_copy(input, output, size, input_gap, output_gap);
            signal_interrupt(system, InterruptId::Ppf);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // SetMemoryFill(start0, value0, end0, start1, value1, end1, control0,
        // control1)
        SET_MEMORY_FILL => {
            for bank in 0..2u32 {
                let start = buffer.get(&mut system.memory, 1 + bank * 3);
                let value = buffer.get(&mut system.memory, 2 + bank * 3);
                let end = buffer.get(&mut system.memory, 3 + bank * 3);
                let control = buffer.get(&mut system.memory, 7 + bank);
                if start == 0 || end <= start {
                    continue;
                }
                let width = fill_width(control);
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
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        // configuration knobs with no effect on us.
        SET_AXI_CONFIG_QOS_MODE
        | SET_PERF_LOG_MODE
        | SET_LED_FORCE_OFF
        | SET_INTERNAL_PRIORITIES
        | SAVE_VRAM_SYS_AREA
        | RESTORE_VRAM_SYS_AREA
        | RESET_GPU_CORE => {
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        GET_PERF_LOG => {
            buffer.reply(&mut system.memory, id, &[0]);
            true
        }
        // RegisterInterruptRelayQueue(flags, event handle)
        //   -> result, thread index, shared memory handle
        REGISTER_INTERRUPT_RELAY_QUEUE => {
            let event = buffer.get(&mut system.memory, 3);
            let (handle, index) = register_interrupt_relay_queue(system, event);
            // 0x2A07 alongside success tells the caller the GPU was already
            // initialized, so it skips its own first-time setup.
            buffer.set(&mut system.memory, 0, Header::new(id, 2, 2).0);
            buffer.set(&mut system.memory, 1, 0x2A07);
            buffer.set(&mut system.memory, 2, index);
            buffer.set(&mut system.memory, 3, Descriptor::handles(1));
            buffer.set(&mut system.memory, 4, handle);
            true
        }
        UNREGISTER_INTERRUPT_RELAY_QUEUE => {
            unregister_interrupt_relay_queue(system);
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        TRY_ACQUIRE_RIGHT | ACQUIRE_RIGHT => {
            system.services.gsp.has_right = true;
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        RELEASE_RIGHT => {
            system.services.gsp.has_right = false;
            buffer.reply(&mut system.memory, id, &[]);
            true
        }
        IMPORT_DISPLAY_CAPTURE_INFO => {
            let info = system.display_capture_info();
            buffer.set(&mut system.memory, 0, Header::new(id, 9, 0).0);
            buffer.set(&mut system.memory, 1, 0);
            for (i, value) in info.iter().enumerate() {
                buffer.set(&mut system.memory, i as u32 + 2, *value);
            }
            true
        }
        _ => false,
    }
}

