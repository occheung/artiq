use core::mem;

pub const DW_EH_PE_omit: u8 = 0xFF;
pub const DW_EH_PE_absptr: u8 = 0x00;

pub const DW_EH_PE_uleb128: u8 = 0x01;
pub const DW_EH_PE_udata2: u8 = 0x02;
pub const DW_EH_PE_udata4: u8 = 0x03;
pub const DW_EH_PE_udata8: u8 = 0x04;
pub const DW_EH_PE_sleb128: u8 = 0x09;
pub const DW_EH_PE_sdata2: u8 = 0x0A;
pub const DW_EH_PE_sdata4: u8 = 0x0B;
pub const DW_EH_PE_sdata8: u8 = 0x0C;

pub const DW_EH_PE_pcrel: u8 = 0x10;
pub const DW_EH_PE_textrel: u8 = 0x20;
pub const DW_EH_PE_datarel: u8 = 0x30;
pub const DW_EH_PE_funcrel: u8 = 0x40;
pub const DW_EH_PE_aligned: u8 = 0x50;

pub const DW_EH_PE_indirect: u8 = 0x80;

pub struct DwarfReader {
    pub ptr: *const u8,
}

#[repr(C, packed)]
struct Unaligned<T>(T);

impl DwarfReader {
    pub fn new(ptr: *const u8) -> DwarfReader {
        DwarfReader { ptr }
    }

    // DWARF streams are packed, so e.g., a u32 would not necessarily be aligned
    // on a 4-byte boundary. This may cause problems on platforms with strict
    // alignment requirements. By wrapping data in a "packed" struct, we are
    // telling the backend to generate "misalignment-safe" code.
    pub unsafe fn read<T: Copy>(&mut self) -> T {
        let Unaligned(result) = *(self.ptr as *const Unaligned<T>);
        self.ptr = self.ptr.add(mem::size_of::<T>());
        result
    }

    pub unsafe fn offset(&mut self, offset: isize) {
        self.ptr = self.ptr.offset(offset);
    }

    // ULEB128 and SLEB128 encodings are defined in Section 7.6 - "Variable
    // Length Data".
    pub unsafe fn read_uleb128(&mut self) -> u64 {
        let mut shift: usize = 0;
        let mut result: u64 = 0;
        let mut byte: u8;
        loop {
            byte = self.read::<u8>();
            result |= ((byte & 0x7F) as u64) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }
        result
    }

    pub unsafe fn read_sleb128(&mut self) -> i64 {
        let mut shift: u32 = 0;
        let mut result: u64 = 0;
        let mut byte: u8;
        loop {
            byte = self.read::<u8>();
            result |= ((byte & 0x7F) as u64) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }
        // sign-extend
        if shift < u64::BITS && (byte & 0x40) != 0 {
            result |= (!0 as u64) << shift;
        }
        result as i64
    }
}

pub struct DwarfWriter {
    pub ptr: *mut u8,
}

impl DwarfWriter {
    pub fn new(ptr: *mut u8) -> DwarfWriter {
        DwarfWriter { ptr }
    }

    pub unsafe fn write<T: Copy>(&mut self, data: T) {
        *(self.ptr as *mut Unaligned<T>) = Unaligned(data);
        self.ptr = self.ptr.add(mem::size_of::<T>());
    }
}

unsafe fn read_encoded_pointer(
    reader: &mut DwarfReader,
    encoding: u8,
) -> Result<usize, ()> {
    if encoding == DW_EH_PE_omit {
        return Err(());
    }

    // DW_EH_PE_aligned implies it's an absolute pointer value
    if encoding == DW_EH_PE_aligned {
        reader.ptr = round_up(reader.ptr as usize, mem::size_of::<usize>())? as *const u8;
        return Ok(reader.read::<usize>());
    }

    match encoding & 0x0F {
        DW_EH_PE_absptr => Ok(reader.read::<usize>()),
        DW_EH_PE_uleb128 => Ok(reader.read_uleb128() as usize),
        DW_EH_PE_udata2 => Ok(reader.read::<u16>() as usize),
        DW_EH_PE_udata4 => Ok(reader.read::<u32>() as usize),
        DW_EH_PE_udata8 => Ok(reader.read::<u64>() as usize),
        DW_EH_PE_sleb128 => Ok(reader.read_sleb128() as usize),
        DW_EH_PE_sdata2 => Ok(reader.read::<i16>() as usize),
        DW_EH_PE_sdata4 => Ok(reader.read::<i32>() as usize),
        DW_EH_PE_sdata8 => Ok(reader.read::<i64>() as usize),
        _ => Err(()),
    }
}

unsafe fn read_encoded_pointer_with_pc(
    reader: &mut DwarfReader,
    encoding: u8,
) -> Result<usize, ()> {
    let original_ptr = reader.ptr;
    let mut result = read_encoded_pointer(reader, encoding)?;

    // DW_EH_PE_aligned implies it's an absolute pointer value
    if encoding == DW_EH_PE_aligned {
        return Ok(result);
    }

    result += match (encoding & 0x70) {
        DW_EH_PE_pcrel => original_ptr as usize,

        // .eh_frame normally would not have these kinds of relocations
        // These would not be supported by a dedicated linker relocation schemes for RISC-V
        DW_EH_PE_textrel | DW_EH_PE_datarel | DW_EH_PE_funcrel | DW_EH_PE_aligned => unimplemented!(),

        // Other values should be impossible
        _ => unreachable!(),
    };

    // result += if (encoding & 0x70) == DW_EH_PE_pcrel {
    //     original_ptr as usize
    // } else {
    //     base
    // };

    if encoding & DW_EH_PE_indirect != 0 {
        result = *(result as *const usize);
    }

    Ok(result)
}

#[inline]
fn round_up(unrounded: usize, align: usize) -> Result<usize, ()> {
    if align.is_power_of_two() {
        Ok((unrounded + align - 1) & !(align - 1))
    } else {
        Err(())
    }
}

// Minimalistic structure to store everything needed for parsing FDEs to synthesize 
// .eh_frame_hdr section. Since we are only linking 1 object file, there should only be 1 call
// frame information (CFI) record, so there should be only 1 common information entry (CIE).
// So the class parses the only CIE on init, cache the encoding info, then parse the FDE on
// iterations based on the cached encoding format.
pub struct EH_Frame {
    // It refers to the augmentation data that corresponds to 'R' in the augmentation string
    pub fde_pointer_encoding: u8,
    pub fde_base: *const u8,
    pub fde_sz: usize,
}

impl EH_Frame {
    pub unsafe fn new(eh_frame_base: *const u8, size: usize) -> EH_Frame {
        let mut cie_reader = DwarfReader::new(eh_frame_base);
        let length = cie_reader.read::<usize>();
        let fde_base = match length {
            // eh_frame with 0 lengths means the CIE is terminated
            // while length == u32::MAX means that the length is only representable with 64 bits,
            // which does not make sense in a system with 32-bit address.
            0 | 0xFFFFFFFF => unimplemented!(),
            _ => cie_reader.ptr.offset(length as isize)
        };
        let fde_sz = size - mem::size_of::<u32>() - length;

        // Routine check on the .eh_frame well-formness, in terms of CIE ID & Version args.
        assert_eq!(cie_reader.read::<u32>(), 0);
        assert_eq!(cie_reader.read::<u8>(), 1);

        // Parse augmentation string
        // The first character must be 'z', there is no way to proceed otherwise
        assert_eq!(cie_reader.read::<u8>(), b'z');
        
        // Establish a pointer that skips ahead of the string
        // Skip code/data alignment factors & return address register along the way as well
        // We only tackle the case where 'z' and 'R' are part of the augmentation string, otherwise
        // we cannot get the addresses to make .eh_frame_hdr
        let mut aug_data_reader = DwarfReader::new(cie_reader.ptr);
        let mut aug_str_len = 0;
        loop {
            if aug_data_reader.read::<u8>() == b'\0' {
                break;
            }
            aug_str_len += 1;
        }
        if aug_str_len == 0 {
            unimplemented!();
        }
        aug_data_reader.read_uleb128(); // Code alignment factor
        aug_data_reader.read_sleb128(); // Data alignment factor
        aug_data_reader.read_uleb128(); // Return address register
        assert_eq!(aug_data_reader.read_uleb128(), 7); // Augmentation data length
        let mut fde_pointer_encoding = DW_EH_PE_omit;
        for i in 0..aug_str_len {
            match cie_reader.read::<u8>() {
                b'L' => {
                    aug_data_reader.read::<u8>();
                },

                b'P' => {
                    let encoding = aug_data_reader.read::<u8>();
                    read_encoded_pointer(&mut aug_data_reader, encoding);
                },

                b'R' => {
                    fde_pointer_encoding = aug_data_reader.read::<u8>();
                },

                // Other characters are not supported
                _ => unimplemented!(),
            }
        }
        assert_ne!(fde_pointer_encoding, DW_EH_PE_omit);

        EH_Frame {
            fde_pointer_encoding,
            fde_base,
            fde_sz,
        }
    }

    pub unsafe fn iterate_fde(&self, callback: &mut dyn FnMut(usize, *const u8)) -> Result<(), ()> {
        // Parse each FDE to obtain the starting address that the FDE applies to
        // Send the FDE offset and the mentioned address to a callback that write up the
        // .eh_frame_hdr section
        let mut remaining_len = self.fde_sz;
        let mut reader = DwarfReader::new(self.fde_base);
        loop {
            if remaining_len == 0 {
                break;
            }

            let fde_ptr = reader.ptr;
            let length = match reader.read::<usize>() {
                0 | 0xFFFFFFFF => unimplemented!(),
                other => other,
            };

            // Remove the length of the header and the content from the counter
            remaining_len -= (length + mem::size_of::<usize>());
            let next_fde_ptr = reader.ptr.offset(length as isize);

            // Skip CIE pointer offset
            reader.read::<usize>();

            // Parse PC Begin using the encoding scheme mentioned in the CIE
            let pc_begin = read_encoded_pointer_with_pc(&mut reader, self.fde_pointer_encoding)?;

            callback(pc_begin, fde_ptr);

            reader.ptr = next_fde_ptr;
        }

        Ok(())
    }
}

pub struct EH_Frame_Hdr {
    fde_writer: DwarfWriter,
    fde_count_ptr: *mut u8,
}

impl EH_Frame_Hdr {
    // Create a EH_Frame_Hdr object, and write out the fixed fields of .eh_frame_hdr to memory
    // .eh_frame_hdr(s) created by this class always use(s) 0x03 encoding (absolute pointer,
    // unsigned 4-bytes)
    pub unsafe fn new(base_ptr: *mut u8, eh_frame_ptr: *const u8) -> EH_Frame_Hdr {
        let mut writer = DwarfWriter::new(base_ptr);
        writer.write::<u8>(1);
        writer.write::<u8>(0x03);
        writer.write::<u8>(0x03);
        writer.write::<u8>(0x03);

        writer.write(eh_frame_ptr);
        let fde_count_ptr = writer.ptr;
        writer.write::<usize>(0);

        EH_Frame_Hdr {
            fde_writer: writer,
            fde_count_ptr: fde_count_ptr,
        }
    }

    pub unsafe fn add_fde(&mut self, init_loc: usize, addr: *const u8) {
        self.fde_writer.write(init_loc);
        self.fde_writer.write(addr);
        *(self.fde_count_ptr) += 1;
    }

    pub unsafe fn size(&self) -> usize {
        12 + *(self.fde_count_ptr) as usize * 8
    }
}
