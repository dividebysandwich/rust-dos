//! COM between processes, as InstallShield 6's setup uses it: a class
//! the registry names a local server for, started with "-Embedding" when
//! a client asks for it, whose class object and objects the client calls
//! through proxies; CoTreatAsClass writing the registry's TreatAs key;
//! the SAFEARRAY functions its engine imports; and a docfile written
//! and read back. Each runs on the interpreter and on the dynamic
//! recompiler. The programs are PE files made with rust-dos-pe's
//! builder, their code assembled with iced.

use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const CORES: [CoreMode; 2] = [CoreMode::Normal, CoreMode::Dynamic];

const HKEY_CLASSES_ROOT: i32 = 0x8000_0000_u32 as i32;
const REG_SZ: i32 = 1;
const CLSCTX_LOCAL_SERVER: i32 = 4;
const REGCLS_MULTIPLEUSE: i32 = 1;
const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;
const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;

/// Where a test program's data and imports are, for its code.
struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> i32 {
        (self.data + offset) as i32
    }

    /// The dword at byte `offset` of the data.
    fn var(&self, offset: u32) -> AsmMemoryOperand {
        dword_ptr((self.data + offset) as u64)
    }

    /// The import address table slot of the function `index`.
    fn import(&self, index: usize) -> AsmMemoryOperand {
        dword_ptr(self.iat[index] as u64)
    }
}

/// A program importing `imports`, with `data` and the code `f` makes.
fn program(
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    let lists: Vec<Vec<Imp>> = imports
        .iter()
        .map(|(_, names)| names.iter().map(|n| Imp::Name(n)).collect())
        .collect();
    let dlls: Vec<(&str, &[Imp])> = imports
        .iter()
        .zip(&lists)
        .map(|((dll, _), list)| (*dll, list.as_slice()))
        .collect();
    let iat = b.imports(&dlls).concat();
    let data_rva = b.section(
        ".data",
        data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let layout = Layout {
        data: BASE + data_rva,
        iat,
    };
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    f(&mut a, &layout).unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &code, flags), code_rva);
    b.entry(code_rva);
    b.build()
}

/// A fresh folder for a test's drive C:, with the program as T.EXE.
fn scratch(name: &str, program: &[u8]) -> PathBuf {
    let base = PathBuf::from("target/test_win32_com_server").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    base
}

/// A machine on `core` with the built-in Windows on and `program`
/// loaded from the shell as C:\T.EXE.
fn machine(test: &str, core: CoreMode, program: &[u8]) -> Box<Cpu> {
    let mut cpu = Cpu::new(scratch(test, program));
    cpu.core = core;
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// Run the machine until the program ends.
fn run_to_exit(cpu: &mut Cpu) {
    for _ in 0..20_000 {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        if run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded {
            return;
        }
    }
    panic!("the program didn't end: {:?}", printed(cpu));
}

/// What the program printed, to standard output and error.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

/// Put `bytes` at `offset` of `data`.
fn put(data: &mut [u8], offset: usize, bytes: &[u8]) {
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
}

/// A GUID whose first field is `d1` and whose last eight bytes are
/// those of COM's own (C0 00 ... 46).
fn guid(d1: u32) -> Vec<u8> {
    let mut g = d1.to_le_bytes().to_vec();
    g.extend([0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
    g
}

/// `text` as a zero-ended UTF-16 string.
fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain([0])
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

/// Call the import `slot` with `args`, pushed last first.
fn call(a: &mut CodeAssembler, slot: AsmMemoryOperand, args: &[i32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg)?;
    }
    a.call(slot)
}

/// Call method `index` of the interface whose pointer is at `at`, with
/// `args` after the interface.
fn call_method(a: &mut CodeAssembler, at: i32, index: i32, args: &[i32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg)?;
    }
    a.mov(eax, dword_ptr(at as u64))?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + 4 * index))
}

/// Set bit `bit` of EBX when EAX is `value`.
fn check_eax(a: &mut CodeAssembler, value: i32, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.cmp(eax, value)?;
    a.jne(skip)?;
    a.or(ebx, 1 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

/// Set bit `bit` of EBX when the dword `at` is `value`.
fn check_var(
    a: &mut CodeAssembler,
    at: AsmMemoryOperand,
    value: i32,
    bit: u32,
) -> Result<(), IcedError> {
    a.mov(eax, at)?;
    check_eax(a, value, bit)
}

/// End the program with EBX as its exit code.
fn exit(a: &mut CodeAssembler, slot: AsmMemoryOperand) -> Result<(), IcedError> {
    a.push(ebx)?;
    a.call(slot)?;
    a.int3()
}

/// A method that returns `result` and takes `args` bytes of arguments,
/// `this` included.
fn method_returning(a: &mut CodeAssembler, result: i32, args: u32) -> Result<(), IcedError> {
    a.mov(eax, result)?;
    a.ret_1(args)
}

#[test]
fn a_local_server_is_started_and_called_from_another_process() {
    // 0x00: the class, 0x10: IID_IClassFactory, 0x20: IID_IDispatch,
    // 0x30: IID_NULL.
    let mut data = vec![0u8; 0x300];
    put(&mut data, 0x00, &guid(0x7E57_C0DE));
    put(&mut data, 0x10, &guid(0x0000_0001));
    put(&mut data, 0x20, &guid(0x0002_0400));
    let key = b"CLSID\\{7E57C0DE-0000-0000-C000-000000000046}\\LocalServer32\0";
    put(&mut data, 0x40, key);
    put(&mut data, 0x90, b"C:\\T.EXE\0");
    // 0xA0: the class factory, 0xA4: the object, 0xA8: the count of
    // type information, 0xAC: the class object's cookie, 0xB0: the
    // name's ID, 0xB4: the list of names, 0xC0: the name.
    put(&mut data, 0xC0, &wide("Zed"));
    // 0xD0: a MSG. 0x200: the server's class factory, 0x204: its
    // object, 0x210: the factory's vtable, 0x240: the object's.
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess", "GetCommandLineA"]),
        (
            "OLE32.dll",
            &[
                "CoInitialize",
                "CoUninitialize",
                "CoGetClassObject",
                "CoRegisterClassObject",
                "CoRevokeClassObject",
            ],
        ),
        ("ADVAPI32.dll", &["RegSetValueA"]),
        (
            "USER32.dll",
            &["GetMessageA", "DispatchMessageA", "PostQuitMessage"],
        ),
    ];
    let code = program(imports, &data.clone(), |a, l| {
        // The vtables need the methods' addresses: the code is assembled
        // once to learn them, then again with them.
        let code = |a: &mut CodeAssembler, at: [u32; 8]| -> Result<[CodeLabel; 8], IcedError> {
            let mut server = a.create_label();
            let mut query = a.create_label();
            let mut add_ref = a.create_label();
            let mut create = a.create_label();
            let mut lock = a.create_label();
            let mut count = a.create_label();
            let mut not_impl = a.create_label();
            let mut ids = a.create_label();
            let mut invoke = a.create_label();

            // The server is started with "-Embedding" after its name.
            a.call(l.import(1))?;
            let mut scan = a.create_label();
            let mut client = a.create_label();
            a.set_label(&mut scan)?;
            a.mov(cl, byte_ptr(eax))?;
            a.inc(eax)?;
            a.cmp(cl, b'-' as i32)?;
            a.je(server)?;
            a.test(cl, cl)?;
            a.jne(scan)?;

            // The client: the class's local server is this program.
            a.set_label(&mut client)?;
            a.xor(ebx, ebx)?;
            call(a, l.import(2), &[0])?;
            call(
                a,
                l.import(7),
                &[HKEY_CLASSES_ROOT, l.at(0x40), REG_SZ, l.at(0x90), 8],
            )?;
            call(
                a,
                l.import(4),
                &[l.at(0), CLSCTX_LOCAL_SERVER, 0, l.at(0x10), l.at(0xA0)],
            )?;
            check_eax(a, 0, 0)?;
            // LockServer(TRUE) returns the server's own success code.
            call_method(a, l.at(0xA0), 4, &[1])?;
            check_eax(a, 0x0004_1234, 1)?;
            // CreateInstance(NULL, IID_IDispatch, &object).
            call_method(a, l.at(0xA0), 3, &[0, l.at(0x20), l.at(0xA4)])?;
            check_eax(a, 0, 2)?;
            // GetTypeInfoCount(&count): the server writes 7.
            call_method(a, l.at(0xA4), 3, &[l.at(0xA8)])?;
            check_eax(a, 0, 3)?;
            check_var(a, l.var(0xA8), 7, 4)?;
            // GetIDsOfNames(IID_NULL, names, 1, 0, &id): the server gives
            // the name's first character and 100h times the count.
            a.mov(l.var(0xB4), l.at(0xC0))?;
            call_method(
                a,
                l.at(0xA4),
                5,
                &[l.at(0x30), l.at(0xB4), 1, 0, l.at(0xB0)],
            )?;
            check_eax(a, 0, 5)?;
            check_var(a, l.var(0xB0), 0x15A, 6)?;
            // Invoke fails in the server, and the client gets its HRESULT.
            call_method(
                a,
                l.at(0xA4),
                6,
                &[0, l.at(0x30), 0, 1, l.at(0x100), 0, 0, 0],
            )?;
            check_eax(a, E_NOTIMPL, 7)?;
            call_method(a, l.at(0xA4), 2, &[])?;
            // LockServer(FALSE) tells the server to end.
            call_method(a, l.at(0xA0), 4, &[0])?;
            call_method(a, l.at(0xA0), 2, &[])?;
            call(a, l.import(3), &[])?;
            exit(a, l.import(0))?;

            // The server: its objects' vtables, its class object registered
            // for other processes, and messages until LockServer(FALSE).
            a.set_label(&mut server)?;
            call(a, l.import(2), &[0])?;
            // The methods' addresses, in the order `code` gives the labels.
            for (i, m) in [0, 1, 1, 2, 3].into_iter().enumerate() {
                a.mov(l.var(0x210 + 4 * i as u32), at[m] as i32)?;
            }
            for (i, m) in [0, 1, 1, 4, 5, 6, 7].into_iter().enumerate() {
                a.mov(l.var(0x240 + 4 * i as u32), at[m] as i32)?;
            }
            a.mov(l.var(0x200), l.at(0x210))?;
            a.mov(l.var(0x204), l.at(0x240))?;
            call(
                a,
                l.import(5),
                &[
                    l.at(0),
                    l.at(0x200),
                    CLSCTX_LOCAL_SERVER,
                    REGCLS_MULTIPLEUSE,
                    l.at(0xAC),
                ],
            )?;
            let mut pump = a.create_label();
            let mut done = a.create_label();
            a.set_label(&mut pump)?;
            call(a, l.import(8), &[l.at(0xD0), 0, 0, 0])?;
            a.test(eax, eax)?;
            a.je(done)?;
            call(a, l.import(9), &[l.at(0xD0)])?;
            a.jmp(pump)?;
            a.set_label(&mut done)?;
            a.push(l.var(0xAC))?;
            a.call(l.import(6))?;
            call(a, l.import(3), &[])?;
            a.xor(ebx, ebx)?;
            exit(a, l.import(0))?;

            // QueryInterface(this, iid, out): this for any interface.
            a.set_label(&mut query)?;
            a.mov(eax, dword_ptr(esp + 12))?;
            a.mov(ecx, dword_ptr(esp + 4))?;
            a.mov(dword_ptr(eax), ecx)?;
            method_returning(a, 0, 12)?;
            // AddRef(this) and Release(this).
            a.set_label(&mut add_ref)?;
            method_returning(a, 1, 4)?;
            // CreateInstance(this, outer, iid, out): the object.
            a.set_label(&mut create)?;
            a.mov(eax, dword_ptr(esp + 16))?;
            a.mov(dword_ptr(eax), l.at(0x204))?;
            method_returning(a, 0, 16)?;
            // LockServer(this, lock): FALSE ends the server.
            a.set_label(&mut lock)?;
            let mut keep = a.create_label();
            a.cmp(dword_ptr(esp + 8), 0)?;
            a.jne(keep)?;
            call(a, l.import(10), &[0])?;
            a.set_label(&mut keep)?;
            method_returning(a, 0x0004_1234, 8)?;
            // GetTypeInfoCount(this, out).
            a.set_label(&mut count)?;
            a.mov(eax, dword_ptr(esp + 8))?;
            a.mov(dword_ptr(eax), 7)?;
            method_returning(a, 0, 8)?;
            // GetTypeInfo(this, index, lcid, out).
            a.set_label(&mut not_impl)?;
            method_returning(a, E_NOTIMPL, 16)?;
            // GetIDsOfNames(this, iid, names, count, lcid, ids).
            a.set_label(&mut ids)?;
            a.mov(eax, dword_ptr(esp + 12))?;
            a.mov(eax, dword_ptr(eax))?;
            a.movzx(eax, word_ptr(eax))?;
            a.mov(ecx, dword_ptr(esp + 16))?;
            a.shl(ecx, 8)?;
            a.add(eax, ecx)?;
            a.mov(ecx, dword_ptr(esp + 24))?;
            a.mov(dword_ptr(ecx), eax)?;
            method_returning(a, 0, 24)?;
            // Invoke(this, id, iid, lcid, flags, params, result, excepinfo,
            // argument).
            a.set_label(&mut invoke)?;
            method_returning(a, E_NOTIMPL, 36)?;
            Ok([query, add_ref, create, lock, count, not_impl, ids, invoke])
        };
        let labels = code(a, [0; 8])?;
        let ip = (l.data + data.len() as u32).next_multiple_of(0x1000) as u64;
        let options = iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS;
        let result = a.assemble_options(ip, options)?;
        let mut at = [0; 8];
        for (i, label) in labels.iter().enumerate() {
            at[i] = result.label_ip(label)? as u32;
        }
        a.reset();
        code(a, at)?;
        Ok(())
    });
    for core in CORES {
        let mut cpu = machine("local_server", core, &code);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {}", printed(&cpu));
    }
}

#[test]
fn co_treat_as_class_writes_the_registrys_treat_as_key() {
    // 0x00: the class, 0x10: the one it is treated as, 0x20: one not
    // registered, 0x30: GUID_NULL, 0x40: CoGetTreatAsClass's answer.
    let mut data = vec![0u8; 0x100];
    put(&mut data, 0x00, &guid(0x7E57_0001));
    put(&mut data, 0x10, &guid(0x7E57_0002));
    put(&mut data, 0x20, &guid(0x7E57_0003));
    put(
        &mut data,
        0x60,
        b"CLSID\\{7E570001-0000-0000-C000-000000000046}\0",
    );
    put(&mut data, 0xA0, b"T\0");
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "OLE32.dll",
            &["CoInitialize", "CoTreatAsClass", "CoGetTreatAsClass"],
        ),
        ("ADVAPI32.dll", &["RegSetValueA"]),
    ];
    let code = program(imports, &data, |a, l| {
        let (treat_as, get_treat_as) = (l.import(2), l.import(3));
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[0])?;
        call(
            a,
            l.import(4),
            &[HKEY_CLASSES_ROOT, l.at(0x60), REG_SZ, l.at(0xA0), 1],
        )?;
        call(a, treat_as, &[l.at(0), l.at(0x10)])?;
        check_eax(a, 0, 0)?;
        call(a, get_treat_as, &[l.at(0), l.at(0x40)])?;
        check_eax(a, 0, 1)?;
        check_var(a, l.var(0x40), 0x7E57_0002, 2)?;
        // GUID_NULL takes the key away: the class is itself again.
        call(a, treat_as, &[l.at(0), l.at(0x30)])?;
        check_eax(a, 0, 3)?;
        call(a, get_treat_as, &[l.at(0), l.at(0x40)])?;
        check_eax(a, 1, 4)?;
        check_var(a, l.var(0x40), 0x7E57_0001, 5)?;
        // A class the registry doesn't have.
        call(a, treat_as, &[l.at(0x20), l.at(0x10)])?;
        check_eax(a, REGDB_E_CLASSNOTREG, 6)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("treat_as", core, &code);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0x7F, "{core:?}: {}", printed(&cpu));
    }
}

#[test]
fn a_safearray_of_bstrs_keeps_copies_of_its_elements() {
    const VT_BSTR: i32 = 8;
    // 0x00: the bounds (3 elements from 1), 0x08: the array, 0x0C: the
    // index, 0x10: the BSTR put, 0x14: the one got back, 0x18: a bound,
    // 0x1C: the copy, 0x40: the text.
    let mut data = vec![0u8; 0x80];
    put(&mut data, 0x00, &[3, 0, 0, 0, 1, 0, 0, 0]);
    put(&mut data, 0x40, &wide("abc"));
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "OLEAUT32.dll",
            &[
                "SafeArrayCreate",
                "SafeArrayPutElement",
                "SafeArrayGetElement",
                "SafeArrayGetUBound",
                "SafeArrayGetLBound",
                "SafeArrayCopy",
                "SafeArrayDestroy",
                "SysAllocString",
                "SysFreeString",
                "SysStringLen",
                "SafeArrayGetDim",
            ],
        ),
    ];
    let code = program(imports, &data, |a, l| {
        let create = l.import(1);
        let (put_element, get_element) = (l.import(2), l.import(3));
        let (ubound, lbound) = (l.import(4), l.import(5));
        let (copy, destroy) = (l.import(6), l.import(7));
        let (alloc, free, len, dims) = (l.import(8), l.import(9), l.import(10), l.import(11));
        a.xor(ebx, ebx)?;
        call(a, create, &[VT_BSTR, 1, l.at(0)])?;
        a.mov(l.var(0x08), eax)?;
        a.test(eax, eax)?;
        let mut made = a.create_label();
        a.je(made)?;
        a.or(ebx, 1)?;
        a.set_label(&mut made)?;
        a.push(l.var(0x08))?;
        a.call(dims)?;
        check_eax(a, 1, 1)?;
        call(a, alloc, &[l.at(0x40)])?;
        a.mov(l.var(0x10), eax)?;
        // Element 3, the last; 4 is past the end.
        a.mov(l.var(0x0C), 3)?;
        a.push(l.var(0x10))?;
        a.push(l.at(0x0C))?;
        a.push(l.var(0x08))?;
        a.call(put_element)?;
        check_eax(a, 0, 2)?;
        a.mov(l.var(0x0C), 4)?;
        a.push(l.var(0x10))?;
        a.push(l.at(0x0C))?;
        a.push(l.var(0x08))?;
        a.call(put_element)?;
        check_eax(a, 0x8002_000B_u32 as i32, 3)?;
        // The array holds its own copy: the program's may go.
        a.push(l.var(0x10))?;
        a.call(free)?;
        a.mov(l.var(0x0C), 3)?;
        a.push(l.at(0x14))?;
        a.push(l.at(0x0C))?;
        a.push(l.var(0x08))?;
        a.call(get_element)?;
        check_eax(a, 0, 4)?;
        a.push(l.var(0x14))?;
        a.call(len)?;
        check_eax(a, 3, 5)?;
        a.push(l.var(0x14))?;
        a.call(free)?;
        a.push(l.at(0x18))?;
        a.push(1)?;
        a.push(l.var(0x08))?;
        a.call(ubound)?;
        check_var(a, l.var(0x18), 3, 6)?;
        a.push(l.at(0x18))?;
        a.push(1)?;
        a.push(l.var(0x08))?;
        a.call(lbound)?;
        check_var(a, l.var(0x18), 1, 7)?;
        a.push(l.at(0x1C))?;
        a.push(l.var(0x08))?;
        a.call(copy)?;
        check_eax(a, 0, 8)?;
        a.push(l.var(0x1C))?;
        a.call(destroy)?;
        a.push(l.var(0x08))?;
        a.call(destroy)?;
        check_eax(a, 0, 9)?;
        // The exit code has eight bits: all ten checks passed is FFh.
        let mut all = a.create_label();
        let mut end = a.create_label();
        a.cmp(ebx, 0x3FF)?;
        a.je(all)?;
        a.and(ebx, 0x7F)?;
        a.jmp(end)?;
        a.set_label(&mut all)?;
        a.mov(ebx, 0xFF)?;
        a.set_label(&mut end)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("safearray", core, &code);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {}", printed(&cpu));
    }
}

#[test]
fn a_docfile_keeps_its_streams_and_classes() {
    const STGM_CREATE_RW: i32 = 0x1012;
    const STGM_RW_EXCLUSIVE: i32 = 0x12;
    const STGM_READ_EXCLUSIVE: i32 = 0x10;
    // 0x00: the file, 0x40: the stream's name, 0x60: its bytes, 0x70:
    // the storage, 0x74: the stream, 0x78: what is read back, 0x80: the
    // count read, 0x90: the class written, 0xA0: the class read.
    let mut data = vec![0u8; 0x100];
    put(&mut data, 0x00, &wide("C:\\T.STG"));
    put(&mut data, 0x40, &wide("Data"));
    put(&mut data, 0x60, b"hello");
    put(&mut data, 0x90, &guid(0x7E57_0010));
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "OLE32.dll",
            &[
                "StgCreateDocfile",
                "StgOpenStorage",
                "StgIsStorageFile",
                "WriteClassStm",
                "ReadClassStm",
            ],
        ),
    ];
    let code = program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[l.at(0), STGM_CREATE_RW, 0, l.at(0x70)])?;
        check_eax(a, 0, 0)?;
        // CreateStream(name, mode, 0, 0, &stream).
        call_method(
            a,
            l.at(0x70),
            3,
            &[l.at(0x40), STGM_RW_EXCLUSIVE, 0, 0, l.at(0x74)],
        )?;
        check_eax(a, 0, 1)?;
        a.push(l.at(0x90))?;
        a.push(l.var(0x74))?;
        a.call(l.import(4))?;
        check_eax(a, 0, 2)?;
        call_method(a, l.at(0x74), 4, &[l.at(0x60), 5, 0])?;
        call_method(a, l.at(0x74), 2, &[])?;
        // The root storage's last Release writes the file.
        call_method(a, l.at(0x70), 2, &[])?;
        call(a, l.import(3), &[l.at(0)])?;
        check_eax(a, 0, 3)?;
        call(
            a,
            l.import(2),
            &[l.at(0), 0, STGM_READ_EXCLUSIVE, 0, 0, l.at(0x70)],
        )?;
        check_eax(a, 0, 4)?;
        // OpenStream(name, 0, mode, 0, &stream).
        call_method(
            a,
            l.at(0x70),
            4,
            &[l.at(0x40), 0, STGM_READ_EXCLUSIVE, 0, l.at(0x74)],
        )?;
        check_eax(a, 0, 5)?;
        a.push(l.at(0xA0))?;
        a.push(l.var(0x74))?;
        a.call(l.import(5))?;
        a.mov(eax, l.var(0xA0))?;
        check_eax(a, 0x7E57_0010, 6)?;
        call_method(a, l.at(0x74), 3, &[l.at(0x78), 8, l.at(0x80)])?;
        check_var(a, l.var(0x80), 5, 7)?;
        call_method(a, l.at(0x74), 2, &[])?;
        call_method(a, l.at(0x70), 2, &[])?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("docfile", core, &code);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {}", printed(&cpu));
        let file = fs::read(PathBuf::from("target/test_win32_com_server/docfile/T.STG"));
        assert!(
            file.is_ok_and(|f| f.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])),
            "{core:?}: the file is a compound file"
        );
    }
}

/// ITypeInfo::Invoke of the IShape object at 0x50 with the DISPPARAMS at
/// 0x90: `count` arguments at 0xB0, the first `named` of them named by
/// the DISPIDs at 0xA0; the result at 0xE0, the EXCEPINFO at 0xF0.
fn invoke_shape(
    a: &mut CodeAssembler,
    l: &Layout,
    memid: i32,
    flags: i32,
    count: i32,
    named: i32,
) -> Result<(), IcedError> {
    a.mov(l.var(0x90), l.at(0xB0))?;
    a.mov(l.var(0x94), l.at(0xA0))?;
    a.mov(l.var(0x98), count)?;
    a.mov(l.var(0x9C), named)?;
    call_method(
        a,
        l.at(0x44),
        11,
        &[
            l.at(0x50),
            memid,
            flags,
            l.at(0x90),
            l.at(0xE0),
            l.at(0xF0),
            l.at(0x110),
        ],
    )
}

#[test]
fn type_info_invoke_calls_a_method_by_its_description() {
    const DISPATCH_METHOD: i32 = 1;
    const DISPATCH_PROPERTYGET: i32 = 2;
    const DISPATCH_PROPERTYPUT: i32 = 4;
    const E_FAIL: i32 = 0x8000_4005_u32 as i32;
    const DISP_E_EXCEPTION: i32 = 0x8002_0009_u32 as i32;
    const DISP_E_BADPARAMCOUNT: i32 = 0x8002_000E_u32 as i32;
    const DISP_E_MEMBERNOTFOUND: i32 = 0x8002_0003_u32 as i32;
    // 0x00: IShape's IID, a dual interface of the test library, 0x10:
    // the library's file, 0x40: the library, 0x44: IShape's ITypeInfo.
    // 0x50: the object (its vtable, then its width and where Move puts
    // dx * 100 + dy), 0x60: its vtable, 0xA0: DISPID_PROPERTYPUT for a
    // put's named argument, 0x120: the name.
    let mut data = vec![0u8; 0x140];
    let mut iid = 0x1111_1111_u32.to_le_bytes().to_vec();
    iid.extend([0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0, 0, 0, 0, 0, 1]);
    put(&mut data, 0x00, &iid);
    put(&mut data, 0x10, &wide("C:\\RDMSFT.TLB"));
    put(&mut data, 0xA0, &(-3i32).to_le_bytes());
    put(&mut data, 0x120, &wide("Zed"));
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        ("OLEAUT32.dll", &["LoadTypeLib", "SysAllocString"]),
    ];
    let code = program(imports, &data.clone(), |a, l| {
        // The vtable needs the methods' addresses: the code is assembled
        // once to learn them, then again with them.
        let code = |a: &mut CodeAssembler, at: [u32; 6]| -> Result<[CodeLabel; 6], IcedError> {
            let mut query = a.create_label();
            let mut add_ref = a.create_label();
            let mut get_width = a.create_label();
            let mut put_width = a.create_label();
            let mut move_by = a.create_label();
            let mut name = a.create_label();

            a.xor(ebx, ebx)?;
            call(a, l.import(1), &[l.at(0x10), l.at(0x40)])?;
            a.mov(esi, eax)?;
            // GetTypeInfoOfGuid: the dispinterface side.
            call_method(a, l.at(0x40), 6, &[l.at(0), l.at(0x44)])?;
            a.or(eax, esi)?;
            check_eax(a, 0, 0)?;
            // IUnknown's, IDispatch's (never called), then IShape's.
            for (i, m) in [0, 1, 1, 1, 1, 1, 1, 2, 3, 4, 5].into_iter().enumerate() {
                a.mov(l.var(0x60 + 4 * i as u32), at[m] as i32)?;
            }
            a.mov(l.var(0x50), l.at(0x60))?;

            // Width = 42, from a VT_I2 named DISPID_PROPERTYPUT.
            a.mov(l.var(0xB0), 2)?;
            a.mov(l.var(0xB8), 42)?;
            invoke_shape(a, l, 1, DISPATCH_PROPERTYPUT, 1, 1)?;
            a.mov(esi, eax)?;
            a.mov(eax, l.var(0x54))?;
            a.sub(eax, 42)?;
            a.or(eax, esi)?;
            check_eax(a, 0, 1)?;
            // Width's get: its [retval] as a VT_I4.
            invoke_shape(a, l, 1, DISPATCH_PROPERTYGET, 0, 0)?;
            a.mov(esi, eax)?;
            a.mov(eax, l.var(0xE0))?;
            a.sub(eax, 3)?;
            a.or(esi, eax)?;
            a.mov(eax, l.var(0xE8))?;
            a.sub(eax, 42)?;
            a.or(eax, esi)?;
            check_eax(a, 0, 2)?;
            // Move(5): dy is its default, 3.
            a.mov(l.var(0xB0), 3)?;
            a.mov(l.var(0xB8), 5)?;
            invoke_shape(a, l, 2, DISPATCH_METHOD, 1, 0)?;
            a.mov(esi, eax)?;
            a.mov(eax, l.var(0x58))?;
            a.sub(eax, 503)?;
            a.or(eax, esi)?;
            check_eax(a, 0, 3)?;
            // Name: a BSTR [retval].
            invoke_shape(a, l, 3, DISPATCH_METHOD, 0, 0)?;
            let mut no_name = a.create_label();
            a.test(eax, eax)?;
            a.jne(no_name)?;
            a.mov(eax, l.var(0xE0))?;
            a.sub(eax, 8)?;
            a.mov(esi, eax)?;
            a.mov(eax, l.var(0xE8))?;
            a.movzx(eax, word_ptr(eax))?;
            a.sub(eax, b'Z' as i32)?;
            a.or(eax, esi)?;
            check_eax(a, 0, 4)?;
            a.set_label(&mut no_name)?;
            // Move(0, 1) fails: DISP_E_EXCEPTION, with its HRESULT.
            a.mov(l.var(0xB0), 3)?;
            a.mov(l.var(0xB8), 1)?;
            a.mov(l.var(0xC0), 3)?;
            a.mov(l.var(0xC8), 0)?;
            invoke_shape(a, l, 2, DISPATCH_METHOD, 2, 0)?;
            a.sub(eax, DISP_E_EXCEPTION)?;
            a.mov(esi, eax)?;
            a.mov(eax, l.var(0xF0 + 28))?;
            a.sub(eax, E_FAIL)?;
            a.or(eax, esi)?;
            check_eax(a, 0, 5)?;
            // Three arguments for two parameters.
            a.mov(l.var(0xD0), 3)?;
            invoke_shape(a, l, 2, DISPATCH_METHOD, 3, 0)?;
            check_eax(a, DISP_E_BADPARAMCOUNT, 6)?;
            invoke_shape(a, l, 99, DISPATCH_METHOD, 0, 0)?;
            check_eax(a, DISP_E_MEMBERNOTFOUND, 7)?;
            exit(a, l.import(0))?;

            // QueryInterface(this, iid, out).
            a.set_label(&mut query)?;
            a.mov(eax, dword_ptr(esp + 12))?;
            a.mov(ecx, dword_ptr(esp + 4))?;
            a.mov(dword_ptr(eax), ecx)?;
            method_returning(a, 0, 12)?;
            // AddRef(this) and Release(this).
            a.set_label(&mut add_ref)?;
            method_returning(a, 1, 4)?;
            // get_Width(this, out).
            a.set_label(&mut get_width)?;
            a.mov(ecx, dword_ptr(esp + 4))?;
            a.mov(eax, dword_ptr(ecx + 4))?;
            a.mov(ecx, dword_ptr(esp + 8))?;
            a.mov(dword_ptr(ecx), eax)?;
            method_returning(a, 0, 8)?;
            // put_Width(this, value).
            a.set_label(&mut put_width)?;
            a.mov(ecx, dword_ptr(esp + 4))?;
            a.mov(eax, dword_ptr(esp + 8))?;
            a.mov(dword_ptr(ecx + 4), eax)?;
            method_returning(a, 0, 8)?;
            // Move(this, dx, dy): E_FAIL for dx 0.
            a.set_label(&mut move_by)?;
            let mut moves = a.create_label();
            a.mov(eax, dword_ptr(esp + 8))?;
            a.test(eax, eax)?;
            a.jne(moves)?;
            method_returning(a, E_FAIL, 12)?;
            a.set_label(&mut moves)?;
            a.imul_3(eax, eax, 100)?;
            a.add(eax, dword_ptr(esp + 12))?;
            a.mov(ecx, dword_ptr(esp + 4))?;
            a.mov(dword_ptr(ecx + 8), eax)?;
            method_returning(a, 0, 12)?;
            // Name(this, out).
            a.set_label(&mut name)?;
            call(a, l.import(2), &[l.at(0x120)])?;
            a.mov(ecx, dword_ptr(esp + 8))?;
            a.mov(dword_ptr(ecx), eax)?;
            method_returning(a, 0, 8)?;
            Ok([query, add_ref, get_width, put_width, move_by, name])
        };
        let labels = code(a, [0; 6])?;
        let ip = (l.data + data.len() as u32).next_multiple_of(0x1000) as u64;
        let options = iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS;
        let result = a.assemble_options(ip, options)?;
        let mut at = [0; 6];
        for (i, label) in labels.iter().enumerate() {
            at[i] = result.label_ip(label)? as u32;
        }
        a.reset();
        code(a, at)?;
        Ok(())
    });
    for core in CORES {
        let mut cpu = machine("invoke", core, &code);
        fs::write(
            "target/test_win32_com_server/invoke/RDMSFT.TLB",
            include_bytes!("../../src/win32/oleaut32/testdata/rdtest-msft.tlb"),
        )
        .unwrap();
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {}", printed(&cpu));
    }
}

#[test]
fn an_empty_safearray_grown_by_redim_holds_its_elements() {
    const VT_VARIANT: i32 = 12;
    // 0x00: the bounds (none from 0), 0x08: the new bounds (3 from 0),
    // 0x10: the array, 0x14: the index, 0x18: the bound, 0x20: the
    // VARIANT put, 0x30: the one got back, 0x40: the elements, 0x44:
    // the task allocator.
    let mut data = vec![0u8; 0x50];
    put(&mut data, 0x08, &[3, 0, 0, 0, 0, 0, 0, 0]);
    put(&mut data, 0x20, &[3, 0, 0, 0, 0, 0, 0, 0, 77, 0, 0, 0]);
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "OLEAUT32.dll",
            &[
                "SafeArrayCreate",
                "SafeArrayRedim",
                "SafeArrayPutElement",
                "SafeArrayGetElement",
                "SafeArrayGetUBound",
                "SafeArrayDestroy",
                "SafeArrayAccessData",
                "SafeArrayUnaccessData",
            ],
        ),
        ("OLE32.dll", &["CoGetMalloc"]),
    ];
    let code = program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[VT_VARIANT, 1, l.at(0)])?;
        a.mov(l.var(0x10), eax)?;
        a.push(l.at(0x08))?;
        a.push(eax)?;
        a.call(l.import(2))?;
        check_eax(a, 0, 0)?;
        a.push(l.at(0x18))?;
        a.push(1)?;
        a.push(l.var(0x10))?;
        a.call(l.import(5))?;
        // The elements' block, from the task allocator, has room for 3.
        a.push(l.at(0x40))?;
        a.push(l.var(0x10))?;
        a.call(l.import(7))?;
        a.push(l.var(0x10))?;
        a.call(l.import(8))?;
        call(a, l.import(9), &[1, l.at(0x44)])?;
        // IMalloc::GetSize(elements).
        a.push(l.var(0x40))?;
        a.mov(eax, l.var(0x44))?;
        a.push(eax)?;
        a.mov(eax, dword_ptr(eax))?;
        a.call(dword_ptr(eax + 4 * 6))?;
        a.cmp(eax, 48)?;
        let mut small = a.create_label();
        a.jb(small)?;
        check_var(a, l.var(0x18), 2, 1)?;
        a.set_label(&mut small)?;
        a.mov(l.var(0x14), 2)?;
        a.push(l.at(0x20))?;
        a.push(l.at(0x14))?;
        a.push(l.var(0x10))?;
        a.call(l.import(3))?;
        check_eax(a, 0, 2)?;
        a.push(l.at(0x30))?;
        a.push(l.at(0x14))?;
        a.push(l.var(0x10))?;
        a.call(l.import(4))?;
        check_eax(a, 0, 3)?;
        check_var(a, l.var(0x30), 3, 4)?;
        check_var(a, l.var(0x38), 77, 5)?;
        a.push(l.var(0x10))?;
        a.call(l.import(6))?;
        check_eax(a, 0, 6)?;
        a.or(ebx, 1 << 7)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("safearray_redim", core, &code);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {}", printed(&cpu));
    }
}

/// SafeArrayDestroyData of a FADF_STATIC array zeroes the program's own
/// block and keeps it, as OLEAUT32 does, rather than freeing it.
#[test]
fn destroying_a_static_safearray_zeroes_its_data_in_place() {
    const FADF_STATIC: i32 = 2;
    // 0x00: the array, 0x10: its 2 elements, the program's own.
    let mut data = vec![0u8; 0x20];
    put(&mut data, 0x10, &[0x55; 8]);
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "OLEAUT32.dll",
            &[
                "SafeArrayAllocDescriptor",
                "SafeArrayDestroyData",
                "SafeArrayDestroyDescriptor",
            ],
        ),
    ];
    let code = program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[1, l.at(0)])?;
        check_eax(a, 0, 0)?;
        a.mov(eax, l.var(0))?;
        a.mov(word_ptr(eax + 2), FADF_STATIC)?;
        a.mov(dword_ptr(eax + 4), 4)?;
        a.mov(dword_ptr(eax + 12), l.at(0x10))?;
        a.mov(dword_ptr(eax + 16), 2)?;
        a.mov(dword_ptr(eax + 20), 0)?;
        a.push(eax)?;
        a.call(l.import(2))?;
        check_eax(a, 0, 1)?;
        check_var(a, l.var(0x10), 0, 2)?;
        check_var(a, l.var(0x14), 0, 3)?;
        // The block stays the array's.
        a.mov(eax, l.var(0))?;
        a.mov(eax, dword_ptr(eax + 12))?;
        check_eax(a, l.at(0x10), 4)?;
        a.push(l.var(0))?;
        a.call(l.import(3))?;
        check_eax(a, 0, 5)?;
        a.or(ebx, 0xC0)?;
        exit(a, l.import(0))
    });
    for core in CORES {
        let mut cpu = machine("safearray_static", core, &code);
        run_to_exit(&mut cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {}", printed(&cpu));
    }
}
