//! Legacy codec throughput and tail latency, built in release.
//!
//! Each family encodes one realistic input with its own encoder, then decodes
//! it repeatedly. Every decode is checked against the input it came from, so
//! a refusal or a short answer fails the run instead of timing as a fast
//! success. One line per codec reports MB/s at the median and the p50/p95/p99
//! latency of one decode.
//!
//! Peak RSS comes from the platform tool wrapping the run (`/usr/bin/time -l`
//! on macOS, `/usr/bin/time -v` on Linux): this crate forbids the `unsafe`
//! that `getrusage` would need, and a guessed figure is worse than none.
//!
//! `--out DIR` also writes the EBCDIC text and the G3 and G4 fax pages, so a
//! system reader (`iconv -f IBM037`, libtiff's `tiffcp -c none`) decodes the
//! same bytes for a matched comparison.
//!
//! ```sh
//! cargo run --locked --release -p lgwks_std --features legacy --example legacy_bench -- --out /tmp/legacy
//! ```

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use lgwks_std::legacy::access::{AccessDb, CellValue, ColumnDef, ColumnTag, DbVersion, rows};
use lgwks_std::legacy::cfb::{CfbFile, build_image, vba};
use lgwks_std::legacy::copybook::{FieldValue, RecordLayout};
use lgwks_std::legacy::decimal::{Decimal, decode_packed, encode_packed};
use lgwks_std::legacy::ebcdic::{CodePage, decode as ebcdic_decode, encode as ebcdic_encode};
use lgwks_std::legacy::fax::{Compression, TiffImage, decode_tiff, encode_tiff};
use lgwks_std::legacy::limits::Limits;

/// What every step of the bench returns.
type BenchResult<T> = Result<T, Box<dyn Error>>;

/// One checked decode, answering how many units it produced.
type Decode<'input> = Box<dyn FnMut() -> BenchResult<usize> + 'input>;

/// The bounds the measurements run under: room for every input below.
const BOUNDS: Limits = Limits::new(67_108_864, 1_048_576, 4096, 128, 134_217_728);

/// Width of an ITU-T T.4 standard fax row, in pixels.
const FAX_WIDTH: u32 = 1728;

/// Rows of a fine-resolution A4 fax page.
const FAX_HEIGHT: u32 = 2200;

/// Decodes per codec: enough samples for a stable p99.
const ROUNDS: usize = 200;

/// Packed values in the decimal case, each `PIC S9(13)V99 COMP-3`.
const PACKED_VALUES: usize = 100_000;

/// Bytes of one 15-digit signed packed value.
const PACKED_WIDTH: usize = 8;

/// Records in the copybook case.
const RECORDS: usize = 10_000;

/// Rows in the database case.
const DB_ROWS: usize = 5_000;

/// One codec's input and its checked decode.
struct Case<'input> {
    /// The codec name printed on the report line.
    name: &'static str,
    /// Bytes one decode reads.
    bytes: usize,
    /// Units one decode must produce: characters, values, records or rows.
    want: usize,
    /// One decode.
    decode: Decode<'input>,
}

/// A xorshift64 stream: fixed-seed, so every run decodes the same bytes.
struct Stream(u64);

impl Stream {
    /// A draw in `0..bound`.
    fn below(&mut self, bound: u64) -> BenchResult<u64> {
        self.0 ^= self.0.wrapping_shl(13);
        self.0 ^= self.0.wrapping_shr(7);
        self.0 ^= self.0.wrapping_shl(17);
        Ok(self
            .0
            .checked_rem(bound)
            .ok_or("a draw needs a positive bound")?)
    }
}

/// A page shaped like typed text: short black glyph runs on rows grouped
/// into lines, with white margins and leading, which is the run profile the
/// T.4 code tables were built for.
fn fax_page(stream: &mut Stream) -> BenchResult<TiffImage> {
    let row_bytes = usize::try_from(FAX_WIDTH.div_ceil(8))?;
    let height = usize::try_from(FAX_HEIGHT)?;
    let mut bitmap = vec![0_u8; row_bytes.saturating_mul(height)];
    let mut glyphs: Vec<(u64, u64)> = Vec::new();
    for (y, row) in bitmap.chunks_mut(row_bytes).enumerate() {
        let line = y.rem_euclid(40);
        if line == 0 {
            glyphs.clear();
            let mut x = 120_u64;
            while x < u64::from(FAX_WIDTH).saturating_sub(120) {
                let width = stream.below(14)?.saturating_add(4);
                glyphs.push((x, width));
                x = x
                    .saturating_add(width)
                    .saturating_add(stream.below(9)?.saturating_add(3));
            }
        }
        if line >= 28 {
            continue;
        }
        for &(start, width) in &glyphs {
            if stream.below(4)? == 0 {
                continue;
            }
            for x in start..start.saturating_add(width) {
                let byte = usize::try_from(x.div_euclid(8))?;
                let mask = 0x80_u8
                    .checked_shr(u32::try_from(x.rem_euclid(8))?)
                    .ok_or("bit past the byte")?;
                if let Some(slot) = row.get_mut(byte) {
                    *slot |= mask;
                }
            }
        }
    }
    Ok(TiffImage::from_bitmap(FAX_WIDTH, FAX_HEIGHT, &bitmap)?)
}

/// Writes the inputs a system reader decodes for the matched comparison.
fn write_fixtures(dir: &Path, ebcdic: &[u8], g3: &[u8], g4: &[u8]) -> BenchResult<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("text.ebcdic"), ebcdic)?;
    std::fs::write(dir.join("page-g3.tif"), g3)?;
    std::fs::write(dir.join("page-g4.tif"), g4)?;
    Ok(())
}

/// The sample at `rank` percent of sorted nanosecond samples, in microseconds.
fn percentile(sorted: &[u128], rank: usize) -> BenchResult<u128> {
    let last = sorted.len().checked_sub(1).ok_or("no samples")?;
    let at = rank
        .saturating_mul(sorted.len())
        .div_ceil(100)
        .saturating_sub(1)
        .min(last);
    Ok(sorted
        .get(at)
        .ok_or("rank past the samples")?
        .div_euclid(1000))
}

/// Runs one case `ROUNDS` times, checking every answer, and prints its line.
fn run(out: &mut impl Write, case: &mut Case<'_>) -> BenchResult<()> {
    let mut samples: Vec<u128> = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let got = (case.decode)()?;
        samples.push(start.elapsed().as_nanos());
        if got != case.want {
            let refusal = format!("{} decoded {got} units, not {}", case.name, case.want);
            #[cfg(feature = "trace")]
            lgwks_std::trace::error!(%refusal, "legacy-bench: a case decoded the wrong unit count");
            return Err(refusal.into());
        }
    }
    samples.sort_unstable();
    let median = samples
        .get(ROUNDS.div_euclid(2))
        .ok_or("no median")?
        .max(&1);
    let centi_mb = u128::try_from(case.bytes)?
        .saturating_mul(100_000)
        .checked_div(*median)
        .ok_or("zero median")?;
    writeln!(
        out,
        "legacy-bench {:<9} {:>9} B  {:>5}.{:02} MB/s  p50 {:>7} us  p95 {:>7} us  p99 {:>7} us",
        case.name,
        case.bytes,
        centi_mb.div_euclid(100),
        centi_mb.rem_euclid(100),
        percentile(&samples, 50)?,
        percentile(&samples, 95)?,
        percentile(&samples, 99)?,
    )?;
    Ok(())
}

/// Reads `--out DIR`, the one flag.
fn fixtures_dir() -> BenchResult<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        if arg != "--out" {
            let refusal = format!("unknown argument {arg:?}; the one flag is --out DIR");
            #[cfg(feature = "trace")]
            lgwks_std::trace::error!(%refusal, "legacy-bench: refusing an argument");
            return Err(refusal.into());
        }
        dir = Some(PathBuf::from(args.next().ok_or("--out needs a directory")?));
    }
    Ok(dir)
}

fn main() -> BenchResult<()> {
    let fixtures = fixtures_dir()?;
    let mut stream = Stream(0x9e37_79b9_7f4a_7c15);

    let text = "PAYROLL 2026-11 DEPT 0042 EMPLOYEE 0001234 NET 0004567.89 ".repeat(18_000);
    let ebcdic = ebcdic_encode(CodePage::Cp037, &text, &BOUNDS)?;

    let mut packed: Vec<u8> = Vec::with_capacity(PACKED_VALUES.saturating_mul(PACKED_WIDTH));
    for index in 0..i128::try_from(PACKED_VALUES)? {
        packed.extend(encode_packed(
            Decimal::new(index.saturating_mul(7919), 2),
            15,
            true,
            &BOUNDS,
        )?);
    }

    let layout = RecordLayout::parse(
        "01 PAY.\n 05 ID PIC X(10).\n 05 NAME PIC X(30).\n 05 NET PIC S9(9)V99 COMP-3.\n 05 HOURS PIC S9(4) COMP.\n",
        &BOUNDS,
    )?;
    let record = layout.encode(
        &[
            FieldValue::Text(String::from("0001234567")),
            FieldValue::Text(format!("{:<30}", "ADA LOVELACE")),
            FieldValue::Number(Decimal::new(456_789, 2)),
            FieldValue::Number(Decimal::new(160, 0)),
        ],
        &BOUNDS,
    )?;
    let records = record.repeat(RECORDS);

    let workbook: Vec<u8> = (0..=250_u8).cycle().take(1_048_576).collect();
    let compound = build_image(&[("Workbook", workbook.as_slice())], &BOUNDS)?;
    let opened = CfbFile::open(&compound, &BOUNDS)?;

    let source = "Sub Total()\r\n    Range(\"A1\").Value = Sum(1, 2)\r\nEnd Sub\r\n".repeat(1_000);
    let vba_coded = vba::compress(source.as_bytes(), &BOUNDS)?;

    let defs = vec![
        ColumnDef::new("Id", ColumnTag::Integer, 0),
        ColumnDef::new("Name", ColumnTag::Text, 40),
        ColumnDef::new("Net", ColumnTag::Currency, 0),
    ];
    let mut table: Vec<Vec<CellValue>> = Vec::with_capacity(DB_ROWS);
    for index in 0..i32::try_from(DB_ROWS)? {
        table.push(vec![
            CellValue::Integer(index),
            CellValue::Text(format!("EMPLOYEE {index:05}")),
            CellValue::Currency(Decimal::new(i128::from(index).saturating_mul(12_345), 4)),
        ]);
    }
    let database = rows::build_image(DbVersion::Jet4, &[("Pay", &defs, &table)], &BOUNDS)?;

    let page = fax_page(&mut stream)?;
    let g3 = encode_tiff(core::slice::from_ref(&page), Compression::G3, &BOUNDS)?;
    let g4 = encode_tiff(core::slice::from_ref(&page), Compression::G4, &BOUNDS)?;
    for (coded, name) in [(&g3, "G3"), (&g4, "G4")] {
        if decode_tiff(coded, &BOUNDS)?.as_slice() != core::slice::from_ref(&page) {
            let refusal = format!("the {name} page did not round-trip");
            #[cfg(feature = "trace")]
            lgwks_std::trace::error!(%refusal, "legacy-bench: a fax page lost bits");
            return Err(refusal.into());
        }
    }
    if let Some(dir) = fixtures.as_deref() {
        write_fixtures(dir, &ebcdic, &g3, &g4)?;
    }

    let record_len = record.len();
    let mut cases = [
        Case {
            name: "ebcdic",
            bytes: ebcdic.len(),
            want: text.len(),
            decode: Box::new(|| Ok(ebcdic_decode(CodePage::Cp037, &ebcdic, &BOUNDS)?.len())),
        },
        Case {
            name: "packed",
            bytes: packed.len(),
            want: PACKED_VALUES,
            decode: Box::new(|| {
                let mut count: usize = 0;
                for chunk in packed.chunks(PACKED_WIDTH) {
                    decode_packed(chunk, 15, 2, true, &BOUNDS)?;
                    count = count.saturating_add(1);
                }
                Ok(count)
            }),
        },
        Case {
            name: "copybook",
            bytes: records.len(),
            want: RECORDS,
            decode: Box::new(|| {
                let mut count: usize = 0;
                for chunk in records.chunks(record_len) {
                    if layout.decode(chunk, &BOUNDS)?.len() == 4 {
                        count = count.saturating_add(1);
                    }
                }
                Ok(count)
            }),
        },
        Case {
            name: "cfb",
            bytes: compound.len(),
            want: workbook.len(),
            decode: Box::new(|| Ok(opened.read_stream("Workbook", &BOUNDS)?.len())),
        },
        Case {
            name: "vba",
            bytes: vba_coded.len(),
            want: source.len(),
            decode: Box::new(|| Ok(vba::decompress(&vba_coded, &BOUNDS)?.len())),
        },
        Case {
            name: "access",
            bytes: database.len(),
            want: DB_ROWS,
            decode: Box::new(|| Ok(AccessDb::open(&database, &BOUNDS)?.rows("Pay")?.len())),
        },
        Case {
            name: "fax-g3",
            bytes: g3.len(),
            want: 1,
            decode: Box::new(|| Ok(decode_tiff(&g3, &BOUNDS)?.len())),
        },
        Case {
            name: "fax-g4",
            bytes: g4.len(),
            want: 1,
            decode: Box::new(|| Ok(decode_tiff(&g4, &BOUNDS)?.len())),
        },
    ];
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for case in &mut cases {
        run(&mut out, case)?;
    }
    Ok(())
}
