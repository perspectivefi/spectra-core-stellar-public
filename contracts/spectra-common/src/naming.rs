// SPDX-License-Identifier: BUSL-1.1
// Pure-function name/symbol generators ported from Solidity NamingUtil.sol.
// The date math is deterministic and bounded for any valid Unix timestamp;
// the workspace's runtime `overflow-checks = true` catches any genuine
// overflow on the rare path where bare arithmetic would matter. Outputs
// (year, month, day) are bounded by domain (year ≤ ~10⁵ for any reachable
// timestamp, month ≤ 12, day ≤ 31), so narrowing casts cannot truncate.
#![allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

use soroban_sdk::{Env, String, contracterror, panic_with_error};

/// Errors raised while building PT/YT/wrapper names and symbols.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum NamingError {
    /// A source token symbol exceeds the fixed metadata buffer.
    SymbolTooLong = 200,
    /// A source token name exceeds the fixed metadata buffer.
    NameTooLong = 201,
}

/// Largest wrapped-vault name that fits `gen_wrapper_name`'s 192-byte buffer
/// after its 31-byte prefix.
const MAX_WRAPPER_NAME_BYTES: usize = 192 - 31;
/// Largest wrapped-vault symbol that fits `gen_wrapper_symbol`'s 64-byte
/// buffer after its 3-byte prefix.
const MAX_WRAPPER_SYMBOL_BYTES: usize = 64 - 3;

fn require_symbol_len(env: &Env, s: &String) {
    if s.len() as usize > MAX_SYMBOL_BYTES {
        panic_with_error!(env, NamingError::SymbolTooLong);
    }
}

// ---------------------------------------------------------------------------
// Constants (from BokkyPooBahsDateTimeLibrary / NamingUtil.sol)
// ---------------------------------------------------------------------------

const SECONDS_PER_DAY: u64 = 86_400;
const OFFSET19700101: u64 = 2_440_588;

// Maximum byte lengths for stack buffers.
// Symbols are short token symbols (e.g. "USDC", "wstETH"). 32 bytes is a
// generous ceiling that covers every real-world case without heap allocation.
const MAX_SYMBOL_BYTES: usize = 32;
// Output buffer: "Principal Token: " (17) + symbol (32) + "(" (1) + symbol (32)
// + ") " (2) + "YYYY/MM/DD" (10) = 94 bytes. 128 gives comfortable headroom.
const MAX_OUTPUT_BYTES: usize = 128;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Port of BokkyPooBah's `_daysToDate`. Solidity uses `int` defensively
/// (the algorithm originated in C) but every intermediate stays non-negative
/// for any valid Unix timestamp, so we can use `u64` directly. The final
/// year/month/day are bounded (year ≤ ~10⁵, month ≤ 12, day ≤ 31), so casting
/// down to `u32` cannot truncate.
fn days_to_date(days: u64) -> (u32, u32, u32) {
    let mut l = days + 68_569 + OFFSET19700101;
    let n = (4 * l) / 146_097;
    l -= (146_097 * n).div_ceil(4);
    let year_raw = (4_000 * (l + 1)) / 1_461_001;
    l = l - (1_461 * year_raw) / 4 + 31;
    let month_raw = (80 * l) / 2_447;
    let day_raw = l - (2_447 * month_raw) / 80;
    let l_inner = month_raw / 11;
    let month_final = month_raw + 2 - 12 * l_inner;
    let year_final = 100 * (n - 49) + year_raw + l_inner;

    (year_final as u32, month_final as u32, day_raw as u32)
}

/// Convert a Unix timestamp to `(year, month, day)`.
fn timestamp_to_date(ts: u64) -> (u32, u32, u32) {
    days_to_date(ts / SECONDS_PER_DAY)
}

// ---------------------------------------------------------------------------
// Buffer-write helpers — all operate on a `&mut [u8; MAX_OUTPUT_BYTES]` and
// advance a cursor index.
// ---------------------------------------------------------------------------

fn write_bytes(buf: &mut [u8; MAX_OUTPUT_BYTES], pos: &mut usize, src: &[u8]) {
    let len = src.len();
    buf[*pos..*pos + len].copy_from_slice(src);
    *pos += len;
}

fn write_str_lit(buf: &mut [u8; MAX_OUTPUT_BYTES], pos: &mut usize, lit: &[u8]) {
    write_bytes(buf, pos, lit);
}

/// Copy a Soroban `String` into the output buffer using a fixed-size
/// intermediate stack buffer.
fn write_soroban_string(buf: &mut [u8; MAX_OUTPUT_BYTES], pos: &mut usize, s: &String) {
    let len = s.len() as usize;
    let mut tmp = [0u8; MAX_SYMBOL_BYTES];
    s.copy_into_slice(&mut tmp[..len]);
    write_bytes(buf, pos, &tmp[..len]);
}

/// Write a `u32` value zero-padded to exactly `width` decimal digits.
fn write_uint_padded(
    buf: &mut [u8; MAX_OUTPUT_BYTES],
    pos: &mut usize,
    mut val: u32,
    width: usize,
) {
    // Build digits right-to-left into a tiny stack buffer (max 4 digits for
    // year; 2 for month/day).
    let mut tmp = [b'0'; 4];
    let mut i = width;
    while i > 0 {
        i -= 1;
        tmp[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    write_bytes(buf, pos, &tmp[..width]);
}

/// Write `"YYYY/MM/DD"` for the given timestamp.
fn write_date(buf: &mut [u8; MAX_OUTPUT_BYTES], pos: &mut usize, ts: u64) {
    let (year, month, day) = timestamp_to_date(ts);
    write_uint_padded(buf, pos, year, 4);
    write_str_lit(buf, pos, b"/");
    write_uint_padded(buf, pos, month, 2);
    write_str_lit(buf, pos, b"/");
    write_uint_padded(buf, pos, day, 2);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `"Principal Token: {ibt}({underlying}) YYYY/MM/DD"`
pub fn gen_pt_name(env: &Env, ibt_sym: &String, underlying_sym: &String, expiry: u64) -> String {
    require_symbol_len(env, ibt_sym);
    require_symbol_len(env, underlying_sym);
    let mut buf = [0u8; MAX_OUTPUT_BYTES];
    let mut pos = 0usize;
    write_str_lit(&mut buf, &mut pos, b"Principal Token: ");
    write_soroban_string(&mut buf, &mut pos, ibt_sym);
    write_str_lit(&mut buf, &mut pos, b"(");
    write_soroban_string(&mut buf, &mut pos, underlying_sym);
    write_str_lit(&mut buf, &mut pos, b") ");
    write_date(&mut buf, &mut pos, expiry);
    String::from_bytes(env, &buf[..pos])
}

/// `"PT-{ibt}({underlying})-YYYY/MM/DD"`
pub fn gen_pt_symbol(env: &Env, ibt_sym: &String, underlying_sym: &String, expiry: u64) -> String {
    require_symbol_len(env, ibt_sym);
    require_symbol_len(env, underlying_sym);
    let mut buf = [0u8; MAX_OUTPUT_BYTES];
    let mut pos = 0usize;
    write_str_lit(&mut buf, &mut pos, b"PT-");
    write_soroban_string(&mut buf, &mut pos, ibt_sym);
    write_str_lit(&mut buf, &mut pos, b"(");
    write_soroban_string(&mut buf, &mut pos, underlying_sym);
    write_str_lit(&mut buf, &mut pos, b")-");
    write_date(&mut buf, &mut pos, expiry);
    String::from_bytes(env, &buf[..pos])
}

/// `"Yield Token: {ibt}({underlying}) YYYY/MM/DD"`
pub fn gen_yt_name(env: &Env, ibt_sym: &String, underlying_sym: &String, expiry: u64) -> String {
    require_symbol_len(env, ibt_sym);
    require_symbol_len(env, underlying_sym);
    let mut buf = [0u8; MAX_OUTPUT_BYTES];
    let mut pos = 0usize;
    write_str_lit(&mut buf, &mut pos, b"Yield Token: ");
    write_soroban_string(&mut buf, &mut pos, ibt_sym);
    write_str_lit(&mut buf, &mut pos, b"(");
    write_soroban_string(&mut buf, &mut pos, underlying_sym);
    write_str_lit(&mut buf, &mut pos, b") ");
    write_date(&mut buf, &mut pos, expiry);
    String::from_bytes(env, &buf[..pos])
}

/// `"YT-{ibt}({underlying})-YYYY/MM/DD"`
pub fn gen_yt_symbol(env: &Env, ibt_sym: &String, underlying_sym: &String, expiry: u64) -> String {
    require_symbol_len(env, ibt_sym);
    require_symbol_len(env, underlying_sym);
    let mut buf = [0u8; MAX_OUTPUT_BYTES];
    let mut pos = 0usize;
    write_str_lit(&mut buf, &mut pos, b"YT-");
    write_soroban_string(&mut buf, &mut pos, ibt_sym);
    write_str_lit(&mut buf, &mut pos, b"(");
    write_soroban_string(&mut buf, &mut pos, underlying_sym);
    write_str_lit(&mut buf, &mut pos, b")-");
    write_date(&mut buf, &mut pos, expiry);
    String::from_bytes(env, &buf[..pos])
}

/// `"Spectra FungibleVault Wrapper: {name}"` — built from the wrapped vault-share
/// token's name. Ported from the Solidity `_wrapperName`. Uses a
/// dedicated buffer (token names can exceed the symbol-sized intermediate used
/// by the date-based builders above).
pub fn gen_wrapper_name(env: &Env, vault_share_name: &String) -> String {
    const PREFIX: &[u8] = b"Spectra FungibleVault Wrapper: ";
    if vault_share_name.len() as usize > MAX_WRAPPER_NAME_BYTES {
        panic_with_error!(env, NamingError::NameTooLong);
    }
    let mut buf = [0u8; 192];
    let mut pos = 0usize;
    write_str_lit_long(&mut buf, &mut pos, PREFIX);
    write_string_long(&mut buf, &mut pos, vault_share_name);
    String::from_bytes(env, &buf[..pos])
}

/// `"sw-{symbol}"` — built from the wrapped vault-share token's symbol.
/// Ported from the Solidity `_wrapperSymbol`.
pub fn gen_wrapper_symbol(env: &Env, vault_share_symbol: &String) -> String {
    const PREFIX: &[u8] = b"sw-";
    if vault_share_symbol.len() as usize > MAX_WRAPPER_SYMBOL_BYTES {
        panic_with_error!(env, NamingError::SymbolTooLong);
    }
    let mut buf = [0u8; 64];
    let mut pos = 0usize;
    write_str_lit_long(&mut buf, &mut pos, PREFIX);
    write_string_long(&mut buf, &mut pos, vault_share_symbol);
    String::from_bytes(env, &buf[..pos])
}

/// Write a literal into an arbitrarily-sized output buffer.
fn write_str_lit_long(buf: &mut [u8], pos: &mut usize, lit: &[u8]) {
    let len = lit.len();
    buf[*pos..*pos + len].copy_from_slice(lit);
    *pos += len;
}

/// Copy a Soroban `String` directly into the output buffer (no fixed-size
/// intermediate, so it accommodates full token names).
fn write_string_long(buf: &mut [u8], pos: &mut usize, s: &String) {
    let len = s.len() as usize;
    s.copy_into_slice(&mut buf[*pos..*pos + len]);
    *pos += len;
}

/// `"YYYY/MM/DD"` for a Unix timestamp.
pub fn timestamp_to_date_string(env: &Env, ts: u64) -> String {
    let mut buf = [0u8; MAX_OUTPUT_BYTES];
    let mut pos = 0usize;
    write_date(&mut buf, &mut pos, ts);
    String::from_bytes(env, &buf[..pos])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::Env;

    fn s(env: &Env, lit: &str) -> String {
        String::from_str(env, lit)
    }

    #[test]
    fn test_timestamp_to_date_string_epoch() {
        let env = Env::default();
        assert_eq!(timestamp_to_date_string(&env, 0), s(&env, "1970/01/01"));
    }

    #[test]
    fn test_timestamp_to_date_string_one_day() {
        let env = Env::default();
        assert_eq!(
            timestamp_to_date_string(&env, 86_400),
            s(&env, "1970/01/02")
        );
    }

    #[test]
    fn test_timestamp_to_date_string_2025_jan_1() {
        let env = Env::default();
        // 2025-01-01 00:00:00 UTC
        assert_eq!(
            timestamp_to_date_string(&env, 1_735_689_600),
            s(&env, "2025/01/01")
        );
    }

    #[test]
    fn test_timestamp_to_date_string_2009_feb_13() {
        let env = Env::default();
        // 1234567890 = 2009-02-13 23:31:30 UTC → date part is 2009/02/13
        assert_eq!(
            timestamp_to_date_string(&env, 1_234_567_890),
            s(&env, "2009/02/13")
        );
    }

    #[test]
    fn test_timestamp_to_date_string_pads_single_digits() {
        let env = Env::default();
        // 2024-03-05 00:00:00 UTC
        assert_eq!(
            timestamp_to_date_string(&env, 1_709_596_800),
            s(&env, "2024/03/05")
        );
    }

    #[test]
    fn test_gen_pt_name() {
        let env = Env::default();
        let ibt = s(&env, "MIBT");
        let und = s(&env, "MUDL");
        let expiry = 1_735_689_600_u64; // 2025-01-01
        assert_eq!(
            gen_pt_name(&env, &ibt, &und, expiry),
            s(&env, "Principal Token: MIBT(MUDL) 2025/01/01")
        );
    }

    #[test]
    #[should_panic]
    fn test_gen_pt_name_rejects_oversized_symbol() {
        let env = Env::default();
        let long = String::from_str(&env, &"A".repeat(MAX_SYMBOL_BYTES + 1));
        let und = s(&env, "MUDL");
        gen_pt_name(&env, &long, &und, 1_735_689_600);
    }

    #[test]
    #[should_panic]
    fn test_gen_wrapper_symbol_rejects_oversized_symbol() {
        let env = Env::default();
        let long = String::from_str(&env, &"A".repeat(MAX_WRAPPER_SYMBOL_BYTES + 1));
        gen_wrapper_symbol(&env, &long);
    }

    #[test]
    fn test_gen_wrapper_symbol_accepts_max_len() {
        let env = Env::default();
        let ok = String::from_str(&env, &"A".repeat(MAX_WRAPPER_SYMBOL_BYTES));
        let out = gen_wrapper_symbol(&env, &ok);
        assert_eq!(out.len() as usize, 3 + MAX_WRAPPER_SYMBOL_BYTES);
    }

    #[test]
    fn test_gen_pt_name_accepts_max_len_symbols() {
        let env = Env::default();
        let ibt = String::from_str(&env, &"A".repeat(MAX_SYMBOL_BYTES));
        let und = String::from_str(&env, &"B".repeat(MAX_SYMBOL_BYTES));
        let out = gen_pt_name(&env, &ibt, &und, 1_735_689_600);
        // "Principal Token: " + 32 + "(" + 32 + ") " + "2025/01/01" = 94
        assert_eq!(out.len() as usize, 94);
    }

    #[test]
    fn test_gen_pt_symbol_accepts_max_len_symbols() {
        let env = Env::default();
        let ibt = String::from_str(&env, &"A".repeat(MAX_SYMBOL_BYTES));
        let und = String::from_str(&env, &"B".repeat(MAX_SYMBOL_BYTES));
        let out = gen_pt_symbol(&env, &ibt, &und, 1_735_689_600);
        // "PT-" + 32 + "(" + 32 + ")-" + "2025/01/01" = 80
        assert_eq!(out.len() as usize, 80);
    }

    #[test]
    fn test_gen_yt_name_accepts_max_len_symbols() {
        let env = Env::default();
        let ibt = String::from_str(&env, &"A".repeat(MAX_SYMBOL_BYTES));
        let und = String::from_str(&env, &"B".repeat(MAX_SYMBOL_BYTES));
        let out = gen_yt_name(&env, &ibt, &und, 1_735_689_600);
        // "Yield Token: " + 32 + "(" + 32 + ") " + "2025/01/01" = 90
        assert_eq!(out.len() as usize, 90);
    }

    #[test]
    fn test_gen_yt_symbol_accepts_max_len_symbols() {
        let env = Env::default();
        let ibt = String::from_str(&env, &"A".repeat(MAX_SYMBOL_BYTES));
        let und = String::from_str(&env, &"B".repeat(MAX_SYMBOL_BYTES));
        let out = gen_yt_symbol(&env, &ibt, &und, 1_735_689_600);
        // "YT-" + 32 + "(" + 32 + ")-" + "2025/01/01" = 80
        assert_eq!(out.len() as usize, 80);
    }

    #[test]
    fn test_gen_pt_symbol() {
        let env = Env::default();
        let ibt = s(&env, "MIBT");
        let und = s(&env, "MUDL");
        let expiry = 1_735_689_600_u64;
        assert_eq!(
            gen_pt_symbol(&env, &ibt, &und, expiry),
            s(&env, "PT-MIBT(MUDL)-2025/01/01")
        );
    }

    #[test]
    fn test_gen_yt_name() {
        let env = Env::default();
        let ibt = s(&env, "MIBT");
        let und = s(&env, "MUDL");
        let expiry = 1_735_689_600_u64;
        assert_eq!(
            gen_yt_name(&env, &ibt, &und, expiry),
            s(&env, "Yield Token: MIBT(MUDL) 2025/01/01")
        );
    }

    #[test]
    fn test_gen_yt_symbol() {
        let env = Env::default();
        let ibt = s(&env, "MIBT");
        let und = s(&env, "MUDL");
        let expiry = 1_735_689_600_u64;
        assert_eq!(
            gen_yt_symbol(&env, &ibt, &und, expiry),
            s(&env, "YT-MIBT(MUDL)-2025/01/01")
        );
    }

    // -----------------------------------------------------------------------
    // Cross-validation vectors
    //
    // Two independent sources of truth:
    //
    //   (1) GNU `date -u -d @<ts> +%Y/%m/%d` — verifies our Gregorian-calendar
    //       output matches POSIX/ISO ground truth, independent of Solidity.
    //
    //   (2) Forge — runs the actual Solidity `NamingUtil` library against the
    //       same inputs and dumps its outputs. Generated by
    //       `core-v2/test/NamingVectors.t.sol`; reproduce with
    //       `cd core-v2 && forge test --match-contract NamingVectors -vv`.
    //
    // Set (1) confirms our port is calendar-correct. Set (2) confirms our
    // port is byte-identical to the Solidity reference implementation. The
    // two sets overlap deliberately — agreement across both is the signal.
    // -----------------------------------------------------------------------

    /// Set (1): GNU `date`-validated date-only vectors. Each row was produced
    /// by `date -u -d @<ts> +%Y/%m/%d` and asserts that our port produces
    /// the same Gregorian output.
    #[test]
    fn test_gnu_date_vectors_timestamp_to_date() {
        let env = Env::default();
        // (unix_timestamp_utc, "YYYY/MM/DD")
        const VECTORS: &[(u64, &str)] = &[
            // Boundaries
            (0, "1970/01/01"),          // epoch
            (86_400, "1970/01/02"),     // +1 day
            (31_535_999, "1970/12/31"), // last second of 1970
            (31_536_000, "1971/01/01"), // first second of 1971
            // Leap years (divisible by 4)
            (1_582_934_400, "2020/02/29"),
            (1_709_164_800, "2024/02/29"),
            (1_709_251_200, "2024/03/01"), // day after 2024 leap day
            // Century leap year (divisible by 400)
            (951_782_400, "2000/02/29"),
            (951_868_800, "2000/03/01"),
            (946_598_400, "1999/12/31"),
            (946_684_800, "2000/01/01"),
            // Century non-leap year (2100 NOT divisible by 400)
            (4_102_444_800, "2100/01/01"),
            (4_107_456_000, "2100/02/28"),
            (4_107_542_400, "2100/03/01"), // skips Feb 29 in 2100
            // Mid-year & well-known reference timestamps
            (1_234_567_890, "2009/02/13"),
            (1_577_836_800, "2020/01/01"),
            (1_734_998_400, "2024/12/24"),
            (1_735_689_600, "2025/01/01"),
            // Intra-day truncation: any second within the same UTC day → same date
            (1_734_998_400 + 86_399, "2024/12/24"),
            // Far future
            (32_503_680_000, "3000/01/01"),
        ];
        for &(ts, expected) in VECTORS {
            let actual = timestamp_to_date_string(&env, ts);
            assert_eq!(actual, s(&env, expected), "ts={}", ts);
        }
    }

    /// Set (2a): Forge-emitted date vectors. Each `(ts, expected)` pair was
    /// produced by running `NamingUtil.timestampToDateString(ts)` inside a
    /// Foundry test on Solidity 0.8.20. Generator at
    /// `core-v2/test/NamingVectors.t.sol::testEmitDateVectors`.
    #[test]
    fn test_forge_vectors_timestamp_to_date() {
        let env = Env::default();
        // Verbatim from forge-test stdout (forge 1.5.1, solc 0.8.20).
        const VECTORS: &[(u64, &str)] = &[
            (0, "1970/01/01"),
            (86_400, "1970/01/02"),
            (31_535_999, "1970/12/31"),
            (31_536_000, "1971/01/01"),
            (1_234_567_890, "2009/02/13"),
            (1_577_836_800, "2020/01/01"),
            (1_582_934_400, "2020/02/29"),
            (1_700_000_000, "2023/11/14"),
            (1_709_164_800, "2024/02/29"),
            (1_709_251_200, "2024/03/01"),
            (1_734_998_400, "2024/12/24"),
            (1_735_689_600, "2025/01/01"),
            (946_598_400, "1999/12/31"),
            (946_684_800, "2000/01/01"),
            (951_782_400, "2000/02/29"),
            (951_868_800, "2000/03/01"),
            (4_102_444_800, "2100/01/01"),
            (4_107_456_000, "2100/02/28"),
            (4_107_542_400, "2100/03/01"),
            (32_503_680_000, "3000/01/01"),
        ];
        for &(ts, expected) in VECTORS {
            let actual = timestamp_to_date_string(&env, ts);
            assert_eq!(actual, s(&env, expected), "ts={}", ts);
        }
    }

    /// Set (2b): Forge-emitted full-naming vectors. Each `(ibt, und, ts,
    /// pt_name, pt_sym, yt_name, yt_sym)` row was produced by calling
    /// `NamingUtil.gen{PT,YT}{Name,Symbol}(...)` in Solidity 0.8.20. The
    /// strings are copied verbatim from forge stdout. Generator at
    /// `core-v2/test/NamingVectors.t.sol::testEmitFullNamingVectors`.
    ///
    /// Coverage: short (1-char) and long (14-char) symbols, all four
    /// generator helpers, leap-day expiries, century leap and non-leap
    /// year boundaries, far-future expiries, and a non-year-end mid-month
    /// expiry (2023-11-14).
    #[test]
    fn test_forge_vectors_full_naming() {
        let env = Env::default();

        struct V {
            ibt: &'static str,
            und: &'static str,
            ts: u64,
            pt_name: &'static str,
            pt_sym: &'static str,
            yt_name: &'static str,
            yt_sym: &'static str,
        }
        let vectors = [
            V {
                ibt: "sDAI",
                und: "DAI",
                ts: 1_735_689_600,
                pt_name: "Principal Token: sDAI(DAI) 2025/01/01",
                pt_sym: "PT-sDAI(DAI)-2025/01/01",
                yt_name: "Yield Token: sDAI(DAI) 2025/01/01",
                yt_sym: "YT-sDAI(DAI)-2025/01/01",
            },
            V {
                ibt: "wstETH",
                und: "ETH",
                ts: 1_709_164_800,
                pt_name: "Principal Token: wstETH(ETH) 2024/02/29",
                pt_sym: "PT-wstETH(ETH)-2024/02/29",
                yt_name: "Yield Token: wstETH(ETH) 2024/02/29",
                yt_sym: "YT-wstETH(ETH)-2024/02/29",
            },
            V {
                ibt: "U",
                und: "X",
                ts: 4_102_444_800,
                pt_name: "Principal Token: U(X) 2100/01/01",
                pt_sym: "PT-U(X)-2100/01/01",
                yt_name: "Yield Token: U(X) 2100/01/01",
                yt_sym: "YT-U(X)-2100/01/01",
            },
            V {
                ibt: "MIBT",
                und: "MUDL",
                ts: 1_734_998_400,
                pt_name: "Principal Token: MIBT(MUDL) 2024/12/24",
                pt_sym: "PT-MIBT(MUDL)-2024/12/24",
                yt_name: "Yield Token: MIBT(MUDL) 2024/12/24",
                yt_sym: "YT-MIBT(MUDL)-2024/12/24",
            },
            V {
                ibt: "aaveUSDC",
                und: "USDC",
                ts: 1_577_836_800,
                pt_name: "Principal Token: aaveUSDC(USDC) 2020/01/01",
                pt_sym: "PT-aaveUSDC(USDC)-2020/01/01",
                yt_name: "Yield Token: aaveUSDC(USDC) 2020/01/01",
                yt_sym: "YT-aaveUSDC(USDC)-2020/01/01",
            },
            V {
                ibt: "crvUSD",
                und: "FRAX",
                ts: 1_700_000_000,
                pt_name: "Principal Token: crvUSD(FRAX) 2023/11/14",
                pt_sym: "PT-crvUSD(FRAX)-2023/11/14",
                yt_name: "Yield Token: crvUSD(FRAX) 2023/11/14",
                yt_sym: "YT-crvUSD(FRAX)-2023/11/14",
            },
            V {
                ibt: "fxS",
                und: "FXS",
                ts: 32_503_680_000,
                pt_name: "Principal Token: fxS(FXS) 3000/01/01",
                pt_sym: "PT-fxS(FXS)-3000/01/01",
                yt_name: "Yield Token: fxS(FXS) 3000/01/01",
                yt_sym: "YT-fxS(FXS)-3000/01/01",
            },
            V {
                ibt: "PTPositionToken",
                und: "STABLE_USD",
                ts: 951_782_400,
                pt_name: "Principal Token: PTPositionToken(STABLE_USD) 2000/02/29",
                pt_sym: "PT-PTPositionToken(STABLE_USD)-2000/02/29",
                yt_name: "Yield Token: PTPositionToken(STABLE_USD) 2000/02/29",
                yt_sym: "YT-PTPositionToken(STABLE_USD)-2000/02/29",
            },
        ];
        for v in &vectors {
            let ibt = s(&env, v.ibt);
            let und = s(&env, v.und);
            assert_eq!(
                gen_pt_name(&env, &ibt, &und, v.ts),
                s(&env, v.pt_name),
                "pt_name {}/{} {}",
                v.ibt,
                v.und,
                v.ts
            );
            assert_eq!(
                gen_pt_symbol(&env, &ibt, &und, v.ts),
                s(&env, v.pt_sym),
                "pt_sym {}/{} {}",
                v.ibt,
                v.und,
                v.ts
            );
            assert_eq!(
                gen_yt_name(&env, &ibt, &und, v.ts),
                s(&env, v.yt_name),
                "yt_name {}/{} {}",
                v.ibt,
                v.und,
                v.ts
            );
            assert_eq!(
                gen_yt_symbol(&env, &ibt, &und, v.ts),
                s(&env, v.yt_sym),
                "yt_sym {}/{} {}",
                v.ibt,
                v.und,
                v.ts
            );
        }
    }
}
