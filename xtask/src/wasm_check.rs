//! A check of the built phone program before it is embedded in the server
//! (capture 0.1.3 start failure, 2026-11-19).
//!
//! wasm-bindgen's JavaScript glue grows and fills the table the program exports as
//! `__wbindgen_externrefs`; the program's own code reads and writes the externref table.
//! In the 0.1.3 release the export named the fixed-size funcref table instead, so
//! every phone failed at `WebAssembly.Table.grow()` before the app started. The code
//! itself still used the right table: `wasm-opt` from Binaryen 108 (Ubuntu 24.04's
//! package, which release.yml installed) rewrote the export's index when it shrank the
//! program. Binaryen 132 keeps it. This module reads the program's
//! import, table and export sections (WebAssembly core specification, binary format)
//! and refuses a program whose `__wbindgen_externrefs` is not a growable externref
//! table.

use std::path::Path;

/// The table the glue grows.
pub const EXTERNREFS: &str = "__wbindgen_externrefs";

const FUNCREF: u8 = 0x70;
const EXTERNREF: u8 = 0x6f;

/// One table of the module: its element type and limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Table {
    elem: u8,
    min: u64,
    max: Option<u64>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn byte(&mut self) -> Result<u8, String> {
        let b = *self
            .bytes
            .get(self.at)
            .ok_or("the program ends in the middle of a section")?;
        self.at += 1;
        Ok(b)
    }

    /// Unsigned LEB128 (at most 64 bits).
    fn uleb(&mut self) -> Result<u64, String> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift >= 64 {
                return Err("a number in the program is too long".into());
            }
            value |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn len(&mut self) -> Result<usize, String> {
        usize::try_from(self.uleb()?).map_err(|_| "a length in the program is too large".into())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .ok_or("the program ends in the middle of a section")?;
        let s = self
            .bytes
            .get(self.at..end)
            .ok_or("the program ends in the middle of a section")?;
        self.at = end;
        Ok(s)
    }

    fn name(&mut self) -> Result<&'a [u8], String> {
        let n = self.len()?;
        self.take(n)
    }

    /// Limits: a flags byte (bit 0: a maximum follows; bit 2: 64-bit), then min and max.
    fn limits(&mut self) -> Result<(u64, Option<u64>), String> {
        let flags = self.byte()?;
        let min = self.uleb()?;
        let max = if flags & 1 == 1 {
            Some(self.uleb()?)
        } else {
            None
        };
        Ok((min, max))
    }

    fn table_type(&mut self) -> Result<Table, String> {
        let elem = self.byte()?;
        let (min, max) = self.limits()?;
        Ok(Table { elem, min, max })
    }
}

/// The tables (imported ones first, as the index space counts them) and the index of
/// the table exported as `name`, if any.
fn tables_and_export(bytes: &[u8], name: &str) -> Result<(Vec<Table>, Option<u64>), String> {
    let body = match (bytes.get(0..4), bytes.get(8..)) {
        (Some(magic), Some(body)) if magic == b"\0asm" => body,
        _ => return Err("not a WebAssembly program".into()),
    };
    let mut r = Reader::new(body);
    let mut tables = Vec::new();
    let mut export = None;
    while !r.done() {
        let id = r.byte()?;
        let size = r.len()?;
        let body = r.take(size)?;
        let mut s = Reader::new(body);
        match id {
            // Imports: only imported tables matter (they come first in the index space).
            2 => {
                for _ in 0..s.uleb()? {
                    s.name()?;
                    s.name()?;
                    match s.byte()? {
                        0 => {
                            s.uleb()?;
                        }
                        1 => tables.push(s.table_type()?),
                        2 => {
                            s.limits()?;
                        }
                        3 => {
                            s.byte()?;
                            s.byte()?;
                        }
                        4 => {
                            s.byte()?;
                            s.uleb()?;
                        }
                        k => return Err(format!("unknown import kind {k}")),
                    }
                }
            }
            4 => {
                for _ in 0..s.uleb()? {
                    tables.push(s.table_type()?);
                }
            }
            7 => {
                for _ in 0..s.uleb()? {
                    let n = s.name()?;
                    let kind = s.byte()?;
                    let index = s.uleb()?;
                    if kind == 1 && n == name.as_bytes() {
                        export = Some(index);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((tables, export))
}

fn describe(t: Table) -> String {
    let elem = match t.elem {
        FUNCREF => "funcref".to_owned(),
        EXTERNREF => "externref".to_owned(),
        other => format!("type 0x{other:02x}"),
    };
    match t.max {
        Some(max) => format!("{elem} table ({} to {max} entries)", t.min),
        None => format!("{elem} table ({} entries, growable)", t.min),
    }
}

/// Check the program's bytes: `__wbindgen_externrefs` must name a growable externref
/// table.
pub fn check_bytes(bytes: &[u8]) -> Result<(), String> {
    let (tables, export) = tables_and_export(bytes, EXTERNREFS)?;
    let index = export.ok_or_else(|| {
        format!(
            "the phone program exports no table named {EXTERNREFS}, which its JavaScript glue needs"
        )
    })?;
    let table = usize::try_from(index)
        .ok()
        .and_then(|i| tables.get(i).copied())
        .ok_or_else(|| {
            format!("the phone program exports {EXTERNREFS} as table {index}, which does not exist")
        })?;
    let advice = "The phone app would fail to start on every phone (WebAssembly.Table.grow fails). \
                  Shrink it with a current wasm-opt (Binaryen 132 or later; Binaryen 108 does this) \
                  or build with --no-wasm-opt to see whether wasm-opt is the cause.";
    if table.elem != EXTERNREF {
        return Err(format!(
            "the phone program exports {EXTERNREFS} as table {index}, a {}, not an externref table. {advice}",
            describe(table)
        ));
    }
    if let Some(max) = table.max
        && max <= table.min
    {
        return Err(format!(
            "the phone program's {EXTERNREFS} (table {index}) is a {} and cannot grow. {advice}",
            describe(table)
        ));
    }
    Ok(())
}

/// Check the program in `dist` (the file Trunk wrote, after wasm-opt).
pub fn check(dist: &Path) -> Result<(), String> {
    let program = dist.join(crate::wasm_opt::PROGRAM);
    let bytes =
        std::fs::read(&program).map_err(|e| format!("cannot read {}: {e}", program.display()))?;
    check_bytes(&bytes).map_err(|e| format!("{}: {e}", program.display()))?;
    println!("==> phone program check: {EXTERNREFS} is a growable externref table");
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn section(id: u8, body: &[u8]) -> Vec<u8> {
        let mut s = vec![id, u8::try_from(body.len()).unwrap()];
        s.extend_from_slice(body);
        s
    }

    /// A small program that passes the check (for other modules' tests).
    pub(crate) fn sample(good: bool) -> Vec<u8> {
        module(u8::from(good), false)
    }

    /// A module with a fixed funcref table (0) and a growable externref table (1),
    /// exporting `__wbindgen_externrefs` as `exported`; `imported_table` adds an
    /// imported table in front of both.
    fn module(exported: u8, imported_table: bool) -> Vec<u8> {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        if imported_table {
            let imp = vec![1, 1, b'e', 1, b't', 1, FUNCREF, 0, 1];
            m.extend(section(2, &imp));
        }
        m.extend(section(
            4,
            &[
                2, FUNCREF, 1, 0x8b, 0x55, 0x8b, 0x55, EXTERNREF, 0, 0x80, 0x08,
            ],
        ));
        let mut exp = vec![1, u8::try_from(EXTERNREFS.len()).unwrap()];
        exp.extend_from_slice(EXTERNREFS.as_bytes());
        exp.extend([1, exported]);
        m.extend(section(7, &exp));
        m
    }

    #[test]
    fn a_growable_externref_export_passes() {
        check_bytes(&module(1, false)).unwrap();
        check_bytes(&module(2, true)).unwrap();
    }

    /// The 0.1.3 defect: the export names the fixed funcref table.
    #[test]
    fn the_funcref_table_under_the_name_fails() {
        let e = check_bytes(&module(0, false)).unwrap_err();
        assert!(e.contains("funcref table (10891 to 10891 entries)"), "{e}");
        assert!(e.contains("not an externref table"), "{e}");
        assert!(e.contains("Binaryen 132"), "{e}");
        // Imported tables shift the index space.
        assert!(check_bytes(&module(1, true)).is_err());
    }

    #[test]
    fn a_fixed_externref_table_fails() {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        m.extend(section(4, &[1, EXTERNREF, 1, 4, 4]));
        let mut exp = vec![1, u8::try_from(EXTERNREFS.len()).unwrap()];
        exp.extend_from_slice(EXTERNREFS.as_bytes());
        exp.extend([1, 0]);
        m.extend(section(7, &exp));
        let e = check_bytes(&m).unwrap_err();
        assert!(e.contains("cannot grow"), "{e}");
    }

    #[test]
    fn missing_or_broken_programs_fail() {
        assert!(check_bytes(b"not wasm").is_err());
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        m.extend(section(4, &[1, EXTERNREF, 0, 1]));
        assert!(check_bytes(&m).unwrap_err().contains("exports no table"));
        let mut cut = module(1, false);
        cut.truncate(cut.len() - 3);
        assert!(check_bytes(&cut).is_err());
        assert!(
            check_bytes(&module(9, false))
                .unwrap_err()
                .contains("does not exist")
        );
    }

    /// The real programs, when given (`MK_WASM_CHECK_GOOD` / `MK_WASM_CHECK_BAD`: paths
    /// to a working build and to the broken 0.1.3 program).
    #[test]
    fn real_programs_when_given() {
        for (var, ok) in [("MK_WASM_CHECK_GOOD", true), ("MK_WASM_CHECK_BAD", false)] {
            let Some(p) = std::env::var_os(var) else {
                eprintln!("{var} not set: NOT CHECKED");
                continue;
            };
            let bytes = std::fs::read(p).unwrap();
            assert_eq!(
                check_bytes(&bytes).is_ok(),
                ok,
                "{var}: {:?}",
                check_bytes(&bytes)
            );
        }
    }
}
