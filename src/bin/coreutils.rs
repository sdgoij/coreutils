// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

use clap::Command;
use coreutils::validation::{self, exit};
use itertools::Itertools as _;
use std::cmp;
use std::ffi::OsString;
use std::io::{self, Write};
use uucore::{Args, error::strip_errno};

// The port's C library is a Rust crate, so the C symbols rustix's libc
// backend, jiff, tempfile and the allocator name are linked from here.
#[cfg(target_os = "minix")]
use minix_libc as _;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// minixrs has no kernel entropy source yet; register a weak RNG so the rand
// users (factor, seq, shuf, sort) can build and run. The backend mixes the
// stack address, a monotonic counter and the cycle counter through SplitMix64.
//
// Two getrandom versions are in the tree: 0.2.x is selected via its `custom`
// feature (enabled in Cargo.toml; only used when no built-in backend matches),
// and 0.4.x is selected via RUSTFLAGS `--cfg getrandom_backend="custom"` on
// the minix build. Both call the symbols defined below.
#[cfg(target_os = "minix")]
mod minix_rng {
    use core::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn splitmix64(mut x: u64) -> impl FnMut() -> u64 {
        move || {
            x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
    }

    pub fn fill(dest: &mut [u8]) -> Result<(), getrandom::Error> {
        let stack = core::ptr::addr_of!(COUNTER) as u64;
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        #[cfg(target_arch = "x86_64")]
        let cycles = unsafe { core::arch::x86_64::_rdtsc() };
        #[cfg(not(target_arch = "x86_64"))]
        let cycles = 0u64;
        let mut next = splitmix64((stack << 32) ^ stack ^ counter ^ cycles);
        for chunk in dest.chunks_mut(8) {
            let x = next();
            for (i, byte) in chunk.iter_mut().enumerate() {
                *byte = (x >> (i * 8)) as u8;
            }
        }
        Ok(())
    }

    getrandom::register_custom_getrandom!(fill);

    #[unsafe(no_mangle)]
    pub unsafe extern "Rust" fn __getrandom_v03_custom(
        dest: *mut u8,
        len: usize,
    ) -> Result<(), getrandom::Error> {
        let dest = unsafe { core::slice::from_raw_parts_mut(dest, len) };
        fill(dest)
    }
}

include!(concat!(env!("OUT_DIR"), "/uutils_map.rs"));

fn usage<T>(utils: &UtilityMap<T>, name: &str) {
    let display_list = utils.keys().copied().join(", ");
    let width = cmp::min(textwrap::termwidth(), 100) - 8; // (opinion/heuristic) max 100 chars wide with 4 character side indentions
    let indent_list = textwrap::indent(&textwrap::fill(&display_list, width), "    ");
    #[cfg(feature = "feat_common_core")]
    let common_core_string = "
Functions:
      '<uutils>' [arguments...]

";
    #[cfg(not(feature = "feat_common_core"))]
    let common_core_string = "";
    let s = format!(
        "{name} {VERSION} (multi-call binary)

Usage: {name} [function [arguments...]]
       {name} --list

{common_core_string}Options:
      --list    lists all defined functions, one per row

Currently defined functions:

{indent_list}"
    );
    if let Err(e) = writeln!(io::stdout(), "{s}")
        && e.kind() != io::ErrorKind::BrokenPipe
    {
        let _ = writeln!(io::stderr(), "coreutils: {}", strip_errno(&e));
        exit(1);
    }
}

#[allow(clippy::cognitive_complexity)]
fn main() {
    let utils = util_map();
    let mut args = uucore::args_os();

    let binary = validation::binary_path(&mut args);
    let binary_as_util = validation::name(&binary).unwrap_or_else(|| {
        usage(&utils, "<unknown binary name>");
        exit(0);
    });

    // binary name ends with util name?
    let is_coreutils = binary_as_util.ends_with("utils");
    let matched_util = utils
        .keys()
        .filter(|&&u| binary_as_util.ends_with(u) && !is_coreutils)
        .max_by_key(|u| u.len()); //Prefer stty more than tty. *utils is not ls

    let util_name = if let Some(&util) = matched_util {
        Some(OsString::from(util))
    } else if is_coreutils || binary_as_util.ends_with("box") {
        // todo: Remove support of "*box" from binary
        uucore::set_utility_is_second_arg();
        args.next()
    } else {
        validation::not_found(&OsString::from(binary_as_util));
    };

    // 0th argument equals util name?
    if let Some(util_os) = util_name {
        let Some(util) = util_os.to_str() else {
            validation::not_found(&util_os)
        };

        match util {
            "--list" => {
                // we should fail with additional args https://github.com/uutils/coreutils/issues/11383#issuecomment-4082564058
                if args.next().is_some() {
                    let _ = writeln!(io::stderr(), "coreutils: invalid argument");
                    exit(1);
                }
                let mut out = io::stdout().lock();
                for util in utils.keys() {
                    if let Err(e) = writeln!(out, "{util}")
                        && e.kind() != io::ErrorKind::BrokenPipe
                    {
                        let _ = writeln!(io::stderr(), "coreutils: {}", strip_errno(&e));
                        exit(1);
                    }
                }
                exit(0);
            }
            "--version" | "-V" => {
                if let Err(e) = writeln!(io::stdout(), "coreutils {VERSION} (multi-call binary)")
                    && e.kind() != io::ErrorKind::BrokenPipe
                {
                    let _ = writeln!(io::stderr(), "coreutils: {}", strip_errno(&e));
                    exit(1);
                }
                exit(0);
            }
            // Not a special command: fallthrough to calling a util
            _ => {}
        }

        if let Some(&(uumain, _)) = utils.get(util) {
            // TODO: plug the deactivation of the translation
            // and load the English strings directly at compilation time in the
            // binary to avoid the load of the flt
            // Could be something like:
            // #[cfg(not(feature = "only_english"))]
            validation::setup_localization_or_exit(util);
            exit(uumain(vec![util_os].into_iter().chain(args)));
        }
        // GNU coreutils --help string shows help for coreutils
        if util == "--help" || util == "-h" {
            usage(&utils, binary_as_util);
            exit(0);
        } else if util.starts_with('-') {
            // Argument looks like an option but wasn't recognized
            validation::unrecognized_option(binary_as_util, &util_os);
        }
        validation::not_found(&util_os);
    }
    // GNU just fails, but busybox tests needs usage
    // todo: patch the test suite instead
    if binary_as_util.ends_with("box") {
        usage(&utils, binary_as_util);
    } else {
        let _ = writeln!(io::stderr(), "coreutils: missing argument");
    }
    exit(1);
}
