//! Hooks x64 réversibles, sans dépendance C.
//!
//! Un saut `E9 rel32` remplace le début de la fonction. Il vise un relais proche
//! qui saute vers le détour. Les instructions écrasées sont relocalisées dans
//! un trampoline. Les autres threads sont gelés pendant chaque écriture.
//!
//! Le trampoline n'est jamais libéré : un appel en cours peut encore s'y trouver.

use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, FlowControl, Instruction,
    InstructionBlock,
};
use std::ffi::c_void;
use std::sync::Mutex;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::{
    FlushInstructionCache, GetThreadContext, CONTEXT, CONTEXT_CONTROL_AMD64,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::Memory::{
    VirtualAlloc, VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_FREE,
    MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, Sleep,
    SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

const JUMP_SIZE: usize = 5;
const BLOCK_SIZE: usize = 4096;
const RELAY_OFFSET: usize = 3000;
const ABSOLUTE_JUMP_SIZE: usize = 14;
const SCAN_BYTES: usize = 64;
const FREEZE_ATTEMPTS: u32 = 32;

/// Blocs (trampoline + relais) alloués par ce module : jamais libérés (un
/// appel peut encore s'y trouver), ils servent à reconnaître un prologue
/// déjà patché par nous.
static OUR_BLOCKS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn owns_address(address: usize) -> bool {
    OUR_BLOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|&block| (block..block + BLOCK_SIZE).contains(&address))
}

/// Hook préparé, ou installé si `installed` est vrai.
pub struct Hook {
    target: usize,
    trampoline: usize,
    relay: usize,
    stolen: usize,
    original: [u8; 16],
    installed: bool,
}

// Les adresses désignent de la mémoire valable toute la vie du processus.
unsafe impl Send for Hook {}

impl Hook {
    /// Adresse à appeler pour exécuter la fonction d'origine.
    pub fn trampoline(&self) -> usize {
        self.trampoline
    }

    /// Prépare un hook sans modifier la fonction cible.
    ///
    /// # Safety
    /// `target` doit être le début d'une fonction chargée et lisible.
    pub unsafe fn prepare(target: usize, detour: usize) -> Result<Self, String> {
        let bytes = unsafe { std::slice::from_raw_parts(target as *const u8, SCAN_BYTES) };
        let (instructions, stolen) = decode_prologue(bytes, target as u64)?;
        // Prologue déjà patché par CE module (hook encore installé) : le
        // relocaliser donnerait un trampoline qui saute vers notre propre
        // relais, donc vers le détour, qui rappellerait le trampoline :
        // récursion infinie dans explorer. Refusé.
        if let Some(first) = instructions.first() {
            if first.flow_control() == FlowControl::UnconditionalBranch
                && owns_address(first.near_branch_target() as usize)
            {
                return Err("fonction déjà détournée par ce module (hook encore installé)".to_owned());
            }
        }

        let block = unsafe { allocate_near(target) }?;
        OUR_BLOCKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(block);
        let encoded = BlockEncoder::encode(
            64,
            InstructionBlock::new(&instructions, block as u64),
            BlockEncoderOptions::NONE,
        )
        .map_err(|e| format!("relocalisation impossible : {e}"))?
        .code_buffer;
        if encoded.len() + ABSOLUTE_JUMP_SIZE > RELAY_OFFSET {
            return Err("prologue relocalisé trop long".to_owned());
        }

        unsafe {
            let base = block as *mut u8;
            std::ptr::copy_nonoverlapping(encoded.as_ptr(), base, encoded.len());
            write_absolute_jump(base.add(encoded.len()), (target + stolen) as u64);
            write_absolute_jump(base.add(RELAY_OFFSET), detour as u64);
            let mut previous = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(
                block as *const c_void,
                BLOCK_SIZE,
                PAGE_EXECUTE_READ,
                &mut previous,
            )
            .map_err(|e| format!("VirtualProtect : {e}"))?;
            let _ = FlushInstructionCache(
                GetCurrentProcess(),
                Some(block as *const c_void),
                BLOCK_SIZE,
            );
        }

        let mut original = [0u8; 16];
        original[..stolen].copy_from_slice(&bytes[..stolen]);
        Ok(Self {
            target,
            trampoline: block,
            relay: block + RELAY_OFFSET,
            stolen,
            original,
            installed: false,
        })
    }

    /// Écrit le saut vers le détour.
    pub fn install(&mut self) -> Result<(), String> {
        if self.installed {
            return Ok(());
        }
        let displacement = self.relay as i64 - (self.target + JUMP_SIZE) as i64;
        let displacement =
            i32::try_from(displacement).map_err(|_| "relais hors de portée".to_owned())?;
        let mut patch = [0x90u8; 16];
        patch[0] = 0xE9;
        patch[1..5].copy_from_slice(&displacement.to_le_bytes());
        self.write(&patch)?;
        self.installed = true;
        Ok(())
    }

    /// Restaure les octets d'origine.
    pub fn uninstall(&mut self) -> Result<(), String> {
        if !self.installed {
            return Ok(());
        }
        let original = self.original;
        self.write(&original)?;
        self.installed = false;
        Ok(())
    }

    /// Écrit `bytes` sur la cible, autres threads gelés.
    ///
    /// Aucune allocation tant que `frozen` est vivant : un thread suspendu
    /// peut détenir le verrou du tas (une allocation ici interbloquerait
    /// explorer). Seul un code d'erreur (`HRESULT`, `Copy`) est conservé
    /// pendant le gel ; le message n'est formaté qu'après la reprise.
    fn write(&self, bytes: &[u8; 16]) -> Result<(), String> {
        let frozen = Frozen::acquire(self.target, self.stolen)?;
        let failure: Option<windows::core::HRESULT> = unsafe {
            let address = self.target as *const c_void;
            let mut previous = PAGE_PROTECTION_FLAGS(0);
            match VirtualProtect(address, self.stolen, PAGE_EXECUTE_READWRITE, &mut previous) {
                Err(e) => Some(e.code()),
                Ok(()) => {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        self.target as *mut u8,
                        self.stolen,
                    );
                    let mut ignored = PAGE_PROTECTION_FLAGS(0);
                    let _ = VirtualProtect(address, self.stolen, previous, &mut ignored);
                    let _ = FlushInstructionCache(GetCurrentProcess(), Some(address), self.stolen);
                    None
                }
            }
        };
        drop(frozen);
        match failure {
            None => Ok(()),
            Some(code) => Err(format!("VirtualProtect : {:#010X}", code.0 as u32)),
        }
    }
}

/// Décode assez d'instructions pour couvrir le saut et refuse les cas dangereux.
fn decode_prologue(bytes: &[u8], ip: u64) -> Result<(Vec<Instruction>, usize), String> {
    let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    let mut length = 0usize;

    while length < JUMP_SIZE {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err("instruction indéchiffrable dans le prologue".to_owned());
        }
        length += instruction.len();
        let flow = instruction.flow_control();
        let terminal = matches!(
            flow,
            FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch
        );
        if flow == FlowControl::Interrupt || flow == FlowControl::Exception {
            return Err("interruption dans le prologue".to_owned());
        }
        instructions.push(instruction);
        if terminal && length < JUMP_SIZE {
            return Err("fonction trop courte pour un saut".to_owned());
        }
    }
    if length > 16 {
        return Err("prologue trop long".to_owned());
    }

    let end = ip + length as u64;
    for instruction in &instructions {
        let is_branch = matches!(
            instruction.flow_control(),
            FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch | FlowControl::Call
        );
        if is_branch {
            let destination = instruction.near_branch_target();
            if destination > ip && destination < end {
                return Err("branchement interne au prologue".to_owned());
            }
        }
    }
    Ok((instructions, length))
}

unsafe fn write_absolute_jump(at: *mut u8, destination: u64) {
    // jmp qword ptr [rip+0] ; dq destination
    unsafe {
        std::ptr::copy_nonoverlapping([0xFFu8, 0x25, 0, 0, 0, 0].as_ptr(), at, 6);
        std::ptr::write_unaligned(at.add(6).cast::<u64>(), destination);
    }
}

/// Réserve un bloc à moins de 2 Gio de la cible.
unsafe fn allocate_near(target: usize) -> Result<usize, String> {
    const GRANULARITY: usize = 0x1_0000;
    const RANGE: usize = 0x7000_0000;
    let aligned = target & !(GRANULARITY - 1);
    let low = aligned.saturating_sub(RANGE).max(GRANULARITY);
    let high = aligned + RANGE;

    let mut distance = GRANULARITY;
    while distance < RANGE {
        for candidate in [aligned.checked_sub(distance), Some(aligned + distance)]
            .into_iter()
            .flatten()
        {
            if !(low..high).contains(&candidate) {
                continue;
            }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
            let queried =
                unsafe { VirtualQuery(Some(candidate as *const c_void), &mut info, size) };
            if queried == 0 || info.State != MEM_FREE {
                continue;
            }
            let block = unsafe {
                VirtualAlloc(
                    Some(candidate as *const c_void),
                    BLOCK_SIZE,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            if !block.is_null() {
                return Ok(block as usize);
            }
        }
        distance += GRANULARITY;
    }
    Err("aucune mémoire libre près de la fonction cible".to_owned())
}

/// Autres threads du processus, suspendus tant que la valeur existe.
struct Frozen {
    threads: Vec<HANDLE>,
}

#[repr(align(16))]
struct AlignedContext(CONTEXT);

impl Frozen {
    /// Suspend les autres threads, sans qu'aucun n'exécute la zone indiquée.
    fn acquire(address: usize, length: usize) -> Result<Self, String> {
        for _ in 0..FREEZE_ATTEMPTS {
            let ids = other_thread_ids()?;
            // La capacité est réservée avant de suspendre : un thread gelé
            // peut détenir le verrou du tas.
            let mut frozen = Frozen {
                threads: Vec::with_capacity(ids.len()),
            };
            let mut clear = true;
            for id in ids {
                let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;
                let Ok(thread) = (unsafe { OpenThread(access, false, id) }) else {
                    continue;
                };
                if unsafe { SuspendThread(thread) } == u32::MAX {
                    unsafe {
                        let _ = CloseHandle(thread);
                    }
                    continue;
                }
                frozen.threads.push(thread);

                let mut context = AlignedContext(unsafe { std::mem::zeroed() });
                context.0.ContextFlags = CONTEXT_CONTROL_AMD64;
                let known = unsafe { GetThreadContext(thread, &mut context.0) }.is_ok();
                let rip = context.0.Rip as usize;
                if !known || (address..address + length).contains(&rip) {
                    clear = false;
                    break;
                }
            }
            if clear {
                return Ok(frozen);
            }
            drop(frozen);
            unsafe { Sleep(1) };
        }
        Err("un thread reste dans la zone à modifier".to_owned())
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        for thread in self.threads.drain(..) {
            unsafe {
                ResumeThread(thread);
                let _ = CloseHandle(thread);
            }
        }
    }
}

fn other_thread_ids() -> Result<Vec<u32>, String> {
    let (process, current) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
        .map_err(|e| format!("instantané des threads : {e}"))?;
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut ids = Vec::with_capacity(256);
    let mut more = unsafe { Thread32First(snapshot, &mut entry) }.is_ok();
    while more {
        if entry.th32OwnerProcessID == process && entry.th32ThreadID != current {
            ids.push(entry.th32ThreadID);
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    Ok(ids)
}
