#![no_std]
#![feature(int_bits_const)]

extern crate byteorder;

use core::{mem, ptr, fmt, slice, str, convert};
use elf::*;
use dwarf::*;
use byteorder::{ByteOrder, LittleEndian};

pub mod elf;
pub mod dwarf;

fn read_unaligned<T: Copy>(data: &[u8], offset: usize) -> Result<T, ()> {
    if data.len() < offset + mem::size_of::<T>() {
        Err(())
    } else {
        let ptr = data.as_ptr().wrapping_offset(offset as isize) as *const T;
        Ok(unsafe { ptr::read_unaligned(ptr) })
    }
}

fn get_ref<T: Copy>(data: &[u8], offset: usize) -> Result<&T, ()> {
    if data.len() < offset + mem::size_of::<T>() {
        Err(())
    } else if (data.as_ptr() as usize + offset) & (mem::align_of::<T>() - 1) != 0 {
        Err(())
    } else {
        let ptr = data.as_ptr().wrapping_offset(offset as isize) as *const T;
        Ok(unsafe { &*ptr })
    }
}

pub fn get_ref_slice<T: Copy>(data: &[u8], offset: usize, len: usize) -> Result<&[T], ()> {
    if data.len() < offset + mem::size_of::<T>() * len {
        Err(())
    } else if (data.as_ptr() as usize + offset) & (mem::align_of::<T>() - 1) != 0 {
        Err(())
    } else {
        let ptr = data.as_ptr().wrapping_offset(offset as isize) as *const T;
        Ok(unsafe { slice::from_raw_parts(ptr, len) })
    }
}

fn name_starting_at_slice(slice: &[u8], offset: usize) -> Result<&[u8], Error> {
    let size = slice.iter().skip(offset).position(|&x| x == 0)
                    .ok_or("symbol in symbol table not null-terminated")?;
    Ok(slice.get(offset..offset + size)
            .ok_or("cannot read symbol name")?)
}

#[derive(Debug)]
pub enum Error<'a> {
    Parsing(&'static str),
    Lookup(&'a [u8])
}

impl<'a> convert::From<&'static str> for Error<'a> {
    fn from(desc: &'static str) -> Error<'a> {
        Error::Parsing(desc)
    }
}

impl<'a> fmt::Display for Error<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &Error::Parsing(desc) =>
                write!(f, "parse error: {}", desc),
            &Error::Lookup(sym) =>
                match str::from_utf8(sym) {
                    Ok(sym) => write!(f, "symbol lookup error: {}", sym),
                    Err(_)  => write!(f, "symbol lookup error: {:?}", sym)
                }
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub enum SectionType {
    RELA(Elf32_Half),
    PROGBITS,
    // Other section types are either directly referenced,
    // or simply not needed for rebinding
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SectionHeader {
    pub sh_index:   Elf32_Half,
    pub sh_type:    SectionType,
    pub sh_off:     usize,
    pub sh_size:    usize,
}

pub struct Library<'a> {
    pub image_off:   Elf32_Word,
    pub image_sz:    usize,
    pub exec_off:    usize,
    pub sectab:      &'a [SectionHeader],
    pub strtab:      &'a [u8],
    pub symtab:      &'a [Elf32_Sym],
    pub eh_frame_sec_ind: usize,
}

impl<'a> Library<'a> {
    pub fn lookup(&self, name: &[u8], allow_local: bool) -> Option<Elf32_Word> {
        for sym in self.symtab {
            let sym_name_off = sym.st_name as usize;
            match name_starting_at_slice(self.strtab, sym_name_off) {
                Ok(sym_name) if sym_name == name => {
                    if !allow_local && (ELF32_ST_BIND(sym.st_info) & STB_GLOBAL == 0) {
                        return None
                    }

                    // // TODO: Remove this branch and simply make it with a lower priority
                    // // than symbols from other sources
                    // if ELF32_ST_BIND(sym.st_info) == STB_WEAK {
                    //     return None
                    // }

                    match sym.st_shndx {
                        SHN_UNDEF => return None,
                        SHN_ABS => return Some(sym.st_value),
                        // The symbol simply refer to some local data
                        sec_ind => {
                            for sec in self.sectab {
                                if sec.sh_index == sec_ind {
                                    return Some(self.image_off + sec.sh_off as u32 + sym.st_value)
                                }
                            }
                            return None
                        }
                    }
                }
                _ => (),
            }
        }

        return None;
    }

    // This is unsafe because it mutates global data (the PLT).
    pub unsafe fn rebind(&self, name: &[u8], addr: Elf32_Word) -> Result<(), Error<'a>> {
        unimplemented!()
    }

    pub fn resolve_rela(&self, rela: &Elf32_Rela, target_section: Elf32_Half, resolve: &dyn Fn(&[u8]) -> Option<Elf32_Word>)
            -> Result<(), Error<'a>> {
        let sym;
        if ELF32_R_SYM(rela.r_info) == 0 {
            sym = None;
        } else {
            sym = Some(self.symtab.get(ELF32_R_SYM(rela.r_info) as usize)
                                  .ok_or("symbol out of bounds of symbol table")?)
        }

        let resolve_symbol_addr = |symbol: Option<&Elf32_Sym>| -> Result<Elf32_Word, Error<'a>> {
            let sym = symbol.ok_or("relocation requires an associated symbol")?;

            // Check if the symbol is defined
            // If so, resolve the symbol immediately
            // If not, try to resolve the symbol using the provided resolve function
            let resolve_sym = || -> Result<Elf32_Word, Error<'a>> {
                let sym_name = name_starting_at_slice(self.strtab, sym.st_name as usize)?;
                match resolve(sym_name) {
                    Some(value) => Ok(value as Elf32_Word),
                    None => {
                        // We couldn't find it anywhere.
                        Err(Error::Lookup(sym_name))
                    }
                }
            };

            match sym.st_shndx {
                SHN_UNDEF => resolve_sym(),
                SHN_ABS => Ok(sym.st_value),
                sec_ind => {
                    for sec in self.sectab {
                        if sec.sh_index == sec_ind {
                            return Ok(self.image_off + sec.sh_off as Elf32_Word + sym.st_value);
                        }
                    }
                    resolve_sym()
                }
            }
        };

        let resolve_target_section_offset = || -> Result<usize, Error<'a>> {
            for sec in self.sectab {
                if sec.sh_index == target_section {
                    return Ok(sec.sh_off);
                }
            }
            Err(Error::Parsing("Cannot find section with matching sh_index"))
        };

        match ELF32_R_TYPE(rela.r_info) {
            R_RISCV_CALL_PLT => {
                let addr = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Modify the auipc+jalr sequence
                // Replaced the sequence with lui+jalr just to makes it cleaner
                unsafe {
                    let auipc_insn_ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u32;
                    let jalr_insn_ptr = auipc_insn_ptr.offset(1);

                    // jalr takes a signed immediate, so increment the upper 20-bits if the lower
                    // 12-bits alone with signed-extension resembles a negative number
                    // Then we can just apply bit-mask on the original lower 12-bits to get the jalr immediate
                    *auipc_insn_ptr = 0b0110111 | (*auipc_insn_ptr & 0xF80) | ((addr + 0x800) & 0xFFFFF000);
                    *jalr_insn_ptr = (*jalr_insn_ptr & 0xFFFFF) | ((addr & 0xFFF) << 20);
                }

                Ok(())
            }

            R_RISCV_GOT_HI20 | R_RISCV_PCREL_HI20 => {
                let addr = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Modify the auipc+lw sequence to get the desired value directly using lui+addi
                // Identical to the one in R_RISCV_CALL_PLT
                unsafe {
                    let auipc_insn_ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u32;
                    let lw_insn_ptr = auipc_insn_ptr.offset(1);

                    *auipc_insn_ptr = 0b0110111 | (*auipc_insn_ptr & 0xF80) | ((addr + 0x800) & 0xFFFFF000);
                    *lw_insn_ptr = 0b0010011 | 0b000 << 12 | (*lw_insn_ptr & 0xF8F80) | ((addr & 0xFFF) << 20);
                }

                Ok(())
            }

            R_RISCV_PCREL_LO12_I => {
                // Already relocated when processing the GOT_HI20/PCREL_HI20 relocation
                Ok(())
            }

            R_RISCV_32 => {
                let value = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Load value to target address
                unsafe {
                    let ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u32;
                    *ptr = value;
                }

                Ok(())
            }

            R_RISCV_ADD32 => {
                let increment = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Add value and addend to target
                unsafe {
                    let ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u8;
                    let bit_slice = core::slice::from_raw_parts_mut(ptr, 4);
                    let old_value = LittleEndian::read_u32(&bit_slice);
                    LittleEndian::write_u32(bit_slice, old_value + increment + rela.r_addend as u32);
                }

                Ok(())
            }

            R_RISCV_SUB32 => {
                let decrement = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Subtract value and addend to target
                unsafe {
                    let ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u8;
                    let bit_slice = core::slice::from_raw_parts_mut(ptr, 4);
                    let old_value = LittleEndian::read_u32(&bit_slice);
                    LittleEndian::write_u32(bit_slice, old_value - decrement - rela.r_addend as u32);
                }

                Ok(())
            }

            R_RISCV_32_PCREL => {
                let abs_value = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;
                let target_pc = self.image_off + target_sec_off as u32 + rela.r_offset;
                let rela_value = abs_value.wrapping_sub(target_pc);

                // Replace with PC-relative value
                unsafe {
                    let ptr = target_pc as *mut u8;
                    let bit_slice = core::slice::from_raw_parts_mut(ptr, 4);

                    bit_slice.copy_from_slice(&rela_value.to_le_bytes());
                }

                Ok(())
            }

            R_RISCV_SET6 => {
                let value = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Set LSB6 of original value with LSB6 of relo value
                unsafe {
                    let ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u8;
                    *ptr = (*ptr & 0xC0) | ((value & 0x3F) as u8);
                }

                Ok(())
            }

            R_RISCV_SUB6 => {
                let value = resolve_symbol_addr(sym)?;
                let target_sec_off = resolve_target_section_offset()?;

                // Subtract LSB6 of original value with LSB6 of relo value
                unsafe {
                    let ptr = (self.image_off + target_sec_off as u32 + rela.r_offset) as *mut u8;
                    *ptr = (*ptr & 0xC0) | ((*ptr & 0x3F) - ((value & 0x3F) as u8) & 0x3F);
                }

                Ok(())
            }

            _ => Err("unsupported relocation type")?
        }
    }

    // Add 2 program headers, one with PT_LOAD, another with PT_GNU_EH_FRAME.
    // An EH frame header (.eh_frame_hdr) will also be added.
    // The PHDR will point to the .eh_frame_hdr section, which gives references to the .eh_frame section.
    pub fn add_eh_frame_support(&self) -> Result<(), Error<'a>> {
        // Fetch .eh_frame from the custom section table
        // TODO: Potentially get rid of the search and just use the offset & size when loading the
        // section headers.
        let mut eh_frame_off = 0;
        let mut eh_frame_sz = 0;
        for sec in self.sectab {
            if sec.sh_index == self.eh_frame_sec_ind as Elf32_Half {
                eh_frame_off = sec.sh_off;
                eh_frame_sz = sec.sh_size;
                break;
            }
        }
        unsafe {
            let eh_frame_ptr = (self.image_off as usize + eh_frame_off) as *const u8;
            let eh_frame = EH_Frame::new(eh_frame_ptr, eh_frame_sz);

            // Append a .eh_frame_hdr section after all existing sections
            // TODO: Enforce 4-bytes alignment for .eh_frame_hdr section
            let eh_frame_hdr_ptr = (self.image_off as usize + self.image_sz) as *mut u8;
            let mut eh_frame_hdr = EH_Frame_Hdr::new(eh_frame_hdr_ptr, eh_frame_ptr);
            let mut fde_callback = |init_pos, addr| eh_frame_hdr.add_fde(init_pos, addr);
            eh_frame.iterate_fde(&mut fde_callback);

            let eh_frame_hdr_sz = eh_frame_hdr.size();

            // Insert a PT_LOAD program header at the beginning of the image
            // Memory space was pre-allocated when loading the library
            let load_offset = mem::size_of::<Elf32_Phdr>() * 2 + mem::size_of::<SectionHeader>() * self.sectab.len();
            let load_sz = self.image_sz - load_offset + eh_frame_hdr_sz;

            let phdr_ptr = self.image_off as *mut Elf32_Phdr;
            *phdr_ptr = Elf32_Phdr {
                p_type: PT_LOAD,
                p_offset: load_offset as Elf32_Off,
                p_vaddr: load_offset as Elf32_Addr,
                p_paddr: load_offset as Elf32_Addr,
                p_filesz: load_sz as Elf32_Word,
                p_memsz: load_sz as Elf32_Word,
                p_flags: (PF_R | PF_X) as Elf32_Word,
                p_align: 4,
            };

            // Append a PT_GNU_EH_FRAME program header after the PT_LOAD header
            // Memory space was pre-allocated when loading the library
            *(phdr_ptr.offset(1)) = Elf32_Phdr {
                p_type: PT_GNU_EH_FRAME,
                p_offset: self.image_sz as Elf32_Off,
                p_vaddr: self.image_sz as Elf32_Addr,
                p_paddr: self.image_sz as Elf32_Addr,
                p_filesz: eh_frame_hdr_sz as Elf32_Word,
                p_memsz: eh_frame_hdr_sz as Elf32_Word,
                p_flags: PF_R as Elf32_Word,
                p_align: 4,
            };
        }

        Ok(())
    }

    pub fn load(data: &[u8], image: &'a mut [u8], resolve: &dyn Fn(&[u8]) -> Option<Elf32_Word>)
            -> Result<Library<'a>, Error<'a>> {

        let ehdr = read_unaligned::<Elf32_Ehdr>(data, 0)
                                  .map_err(|()| "cannot read ELF header")?;

        const IDENT: [u8; EI_NIDENT] = [
            ELFMAG0,    ELFMAG1,     ELFMAG2,    ELFMAG3,
            ELFCLASS32, ELFDATA2LSB, EV_CURRENT, ELFOSABI_NONE,
            /* ABI version */ 0, /* padding */ 0, 0, 0, 0, 0, 0, 0
        ];

        #[cfg(target_arch = "riscv32")]
        const ARCH: u16 = EM_RISCV;
        #[cfg(not(target_arch = "riscv32"))]
        const ARCH: u16 = EM_NONE;

        #[cfg(all(target_feature = "f", target_feature = "d"))]
        const FLAGS: u32 = EF_RISCV_FLOAT_ABI_DOUBLE;

        #[cfg(not(all(target_feature = "f", target_feature = "d")))]
        const FLAGS: u32 = EF_RISCV_FLOAT_ABI_SOFT;

        if ehdr.e_ident != IDENT || ehdr.e_type != ET_REL || ehdr.e_machine != ARCH || ehdr.e_flags != FLAGS {
            return Err("not a shared library for current architecture")?
        }

        let shdrs = get_ref_slice::<Elf32_Shdr>(data, ehdr.e_shoff as usize, ehdr.e_shnum as usize)
                                  .map_err(|()| "cannot read section header table")?;

        // Prepare the string table (strtab)
        // It is to locate the .eh_frame for generating .eh_frame_hdr
        // after its relocation
        let strtab_shdr = shdrs[ehdr.e_shstrndx as usize];
        let strtab_src = get_ref_slice::<u8>(data, strtab_shdr.sh_offset as usize, strtab_shdr.sh_size as usize)
                                   .map_err(|()| "cannot read string table from data")?;

        // Reserve 2 program headers (PHDR) at the start of the image
        // The 2 sections are a generic PT_LOAD and PT_GNU_EH_FRAME. This is to
        // provide just enough information to libunwind to process the exception
        // handling info. The PT_LOAD PHDR will span the entire image, where the 
        // PT_GNU_EH_FRAME PHDR will span the future .eh_frame_hdr.
        // The content is to be defined after relocation is complete.
        // There isn't really any reason to not align the pointer, as the image
        // address should be well-aligned (e.g. 0x45060000)
        let sectab_off = mem::size_of::<Elf32_Phdr>() * 2;

        // Pre-calculate copied image size to allocate section table
        // Only include PROGBITS and RELA (maybe not even RELA)
        let sectab_sz = shdrs.iter()
                             .filter(|shdr| (shdr.sh_type == SHT_PROGBITS as u32 || shdr.sh_type == SHT_RELA as u32) && shdr.sh_size != 0)
                             .count();
        let sectab = get_ref_slice::<SectionHeader>(image, sectab_off, sectab_sz)
                                   .map_err(|()| "cannot read section header table")?;

        // Load all PROGBITS, RELA, SYMTAB sections to target
        let mut load_off = sectab_off + sectab_sz * mem::size_of::<SectionHeader>();
        let mut sectab_ind = 0;

        // Get the loaded offset and sizes of special sections
        let mut exec_sec_off = 0;
        let (mut exec_off, mut exec_sz) = (0, 0);
        let (mut symtab_off, mut symtab_sz) = (0, 0);
        let (mut strtab_off, mut strtab_sz) = (0, 0);
        let mut eh_frame_sec_ind = 0;

        for (i, shdr) in shdrs.iter().enumerate() {
            let shdr_sz = shdr.sh_size as usize;
            let shdr_off = shdr.sh_offset as usize;
            let shdr_addralign = shdr.sh_addralign as usize;
            let shdr_flags = shdr.sh_flags as usize;

            if shdr_sz == 0 {
                continue;
            }

            macro_rules! load_section_from_data {
                () => {
                    {
                        // Enforce section alignment
                        load_off += (shdr_addralign - (load_off % shdr_addralign)) % shdr_addralign;

                        let dest = image.get_mut(load_off..load_off+shdr_sz)
                                        .ok_or("cannot write to section header destination")?;
                        let src = data.get(shdr_off..shdr_off+shdr_sz)
                                    .ok_or("cannot read from section header destination")?;
                        dest.copy_from_slice(src);

                        // Increment dest offset
                        let loaded_section_off = load_off;
                        load_off += shdr_sz;
                        loaded_section_off
                    }
                };
            }

            macro_rules! add_section_to_sectab {
                ($entry: expr) => {
                    unsafe {
                        let sectab_entry = (image.as_mut_ptr().offset(sectab_off as isize) as *mut SectionHeader).offset(sectab_ind as isize);
                        *sectab_entry = $entry;
                        sectab_ind += 1;
                    }
                };
            }

            match shdr.sh_type as usize {
                SHT_PROGBITS => {
                    let offset = load_section_from_data!();
                    add_section_to_sectab!(
                        SectionHeader {
                            sh_index:   i as Elf32_Half,
                            sh_type:    SectionType::PROGBITS,
                            sh_off:     offset,
                            sh_size:    shdr_sz,
                        }
                    );
                    if shdr_flags & SHF_EXECINSTR != 0 {
                        exec_off = offset;
                        exec_sz = shdr_sz;
                        exec_sec_off = i;
                    }
                    else if name_starting_at_slice(strtab_src, shdr.sh_name as usize).unwrap() == ".eh_frame".as_bytes() {
                        eh_frame_sec_ind = i;
                    }
                }

                SHT_SYMTAB => {
                    symtab_off = load_section_from_data!();
                    symtab_sz = shdr_sz / mem::size_of::<Elf32_Sym>();
                }

                SHT_STRTAB => {
                    strtab_off = load_section_from_data!();
                    strtab_sz = shdr_sz;
                }

                SHT_RELA => {
                    let offset = load_section_from_data!();
                    add_section_to_sectab!(
                        SectionHeader {
                            sh_index:   i as Elf32_Half,
                            sh_type:    SectionType::RELA(shdr.sh_info as Elf32_Half),
                            sh_off:     offset,
                            sh_size:    shdr_sz,
                        }
                    );
                }

                _ => ()
            }
        }

        // Forget data
        mem::drop(data);

        // Drop the mutability. See also the comment below.
        let image = &*image;

        let sectab = get_ref_slice::<SectionHeader>(image, sectab_off, sectab_sz)
                                   .map_err(|()| "cannot read section header table")?;
        let symtab = get_ref_slice::<Elf32_Sym>(image, symtab_off, symtab_sz)
                                   .map_err(|()| "cannot read symbol table")?;
        let strtab = get_ref_slice::<u8>(image, strtab_off, strtab_sz)
                                   .map_err(|()| "cannot read string table")?;

        let library = Library {
            image_off:   image.as_ptr() as Elf32_Word,
            image_sz:    load_off,
            exec_off:    exec_off,
            strtab:      strtab,
            symtab:      symtab,
            sectab:      sectab,
            eh_frame_sec_ind: eh_frame_sec_ind,
        };

        // If a borrow exists anywhere, the borrowed memory cannot be mutated except
        // through that pointer or it's UB. However, we need to retain pointers
        // to the symbol tables and relocations, and at the same time mutate the code
        // to resolve the relocations.
        //
        // To avoid invoking UB, we drop the only pointer to the entire area (which is
        // unique since it's a &mut); we retain pointers to the various tables, but
        // we never write to the memory they refer to, so it's safe.
        mem::drop(image);

        for sec in library.sectab {
            if let SectionType::RELA(resolve_ind) = sec.sh_type {
                let rela = get_ref_slice::<Elf32_Rela>(image, sec.sh_off, sec.sh_size / mem::size_of::<Elf32_Rela>()).unwrap();
                for r in rela { library.resolve_rela(r, resolve_ind, resolve)? }
            }
        }

        // Add PH_LOAD, PT_GNU_EH_FRAME & .eh_frame_hdr
        library.add_eh_frame_support();

        Ok(library)
    }
}
