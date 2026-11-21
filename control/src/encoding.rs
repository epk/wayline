use anyhow::{Result, bail};
use arrow_array::{ArrayRef, Float64Array, Int32Array, Int64Array, RecordBatch, UInt32Array};
use arrow_ipc::CompressionType;
use arrow_ipc::writer::{IpcWriteOptions, StreamWriter};
use arrow_schema::{DataType, Field, Schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Arc;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub enum Column {
    F64(Vec<Option<f64>>),
    I32(Vec<Option<i32>>),
    U32(Vec<u32>),
    I64(Vec<Option<i64>>),
}

pub struct Frame {
    pub meta: Map<String, Value>,
    pub columns: Vec<(String, Column)>,
}

pub trait Document: Serialize {
    fn frame(&self) -> Result<Frame>;
}

#[derive(Default, Deserialize)]
struct Request {
    #[serde(default)]
    format: Option<String>,
}

pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let group = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        for (position, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if position <= chunk.len() {
                out.push(char::from(
                    ALPHABET[usize::try_from(group >> shift & 63).unwrap_or(0)],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn ipc(columns: Vec<(String, Column)>) -> Result<Vec<u8>> {
    let mut fields = Vec::with_capacity(columns.len());
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for (name, column) in columns {
        let (kind, nullable, array): (DataType, bool, ArrayRef) = match column {
            Column::F64(values) => (
                DataType::Float64,
                true,
                Arc::new(Float64Array::from(values)),
            ),
            Column::I32(values) => (DataType::Int32, true, Arc::new(Int32Array::from(values))),
            Column::U32(values) => (DataType::UInt32, false, Arc::new(UInt32Array::from(values))),
            Column::I64(values) => (DataType::Int64, true, Arc::new(Int64Array::from(values))),
        };
        fields.push(Field::new(name, kind, nullable));
        arrays.push(array);
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(Arc::clone(&schema), arrays)?;
    let mut writer = StreamWriter::try_new_with_options(
        Vec::new(),
        &schema,
        IpcWriteOptions::default().try_with_compression(Some(CompressionType::ZSTD))?,
    )?;
    writer.write(&batch)?;
    writer.finish()?;
    Ok(writer.into_inner()?)
}

pub fn reply(raw: &str, document: &impl Document) -> Result<Value> {
    let request: Request = serde_json::from_str(raw).unwrap_or_default();
    match request.format.as_deref() {
        None => Ok(serde_json::to_value(document)?),
        Some("arrow") => {
            let Frame { mut meta, columns } = document.frame()?;
            meta.insert("format".into(), Value::String("arrow".into()));
            meta.insert("data".into(), Value::String(base64(&ipc(columns)?)));
            Ok(Value::Object(meta))
        }
        Some(other) => bail!("Unknown format {other}."),
    }
}

#[cfg(test)]
pub mod tests {
    use super::{Column, base64, ipc};
    use anyhow::{Context, Result, ensure};
    use arrow_array::{Array, Float64Array, Int32Array, RecordBatch, UInt32Array};
    use arrow_ipc::reader::StreamReader;
    use std::io::Cursor;

    pub fn decode(text: &str) -> Vec<u8> {
        let value = |byte: u8| -> u32 {
            super::ALPHABET
                .iter()
                .position(|candidate| *candidate == byte)
                .and_then(|index| u32::try_from(index).ok())
                .unwrap_or(0)
        };
        let mut out = Vec::new();
        for chunk in text.as_bytes().chunks(4) {
            let group = chunk
                .iter()
                .fold(0, |group, byte| group << 6 | value(*byte));
            let bytes = group.to_be_bytes();
            let kept = chunk.iter().filter(|byte| **byte != b'=').count() - 1;
            out.extend_from_slice(&bytes[1..=kept]);
        }
        out
    }

    pub fn batch(data: &str) -> Result<RecordBatch> {
        StreamReader::try_new(Cursor::new(decode(data)), None)?
            .next()
            .context("The stream has no batch.")?
            .map_err(Into::into)
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
            assert_eq!(decode(encoded), plain.as_bytes());
        }
    }

    #[test]
    fn frames_round_trip_through_compressed_ipc() -> Result<()> {
        let data = ipc(vec![
            (
                "0.median".into(),
                Column::I32(vec![Some(20_000), None, Some(31_500)]),
            ),
            ("0.sent".into(), Column::U32(vec![4, 0, 4])),
            (
                "signal".into(),
                Column::F64(vec![None, Some(-61.5), Some(-60.0)]),
            ),
        ])?;
        let found = batch(&base64(&data))?;
        ensure!(found.num_rows() == 3);
        let median = found
            .column_by_name("0.median")
            .and_then(|column| column.as_any().downcast_ref::<Int32Array>())
            .context("median")?;
        ensure!(median.value(0) == 20_000 && median.is_null(1));
        let sent = found
            .column_by_name("0.sent")
            .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
            .context("sent")?;
        ensure!(sent.values().to_vec() == [4, 0, 4]);
        let signal = found
            .column_by_name("signal")
            .and_then(|column| column.as_any().downcast_ref::<Float64Array>())
            .context("signal")?;
        ensure!(signal.is_null(0) && (signal.value(1) + 61.5).abs() < f64::EPSILON);
        let plain = ipc(vec![("x".into(), Column::U32(vec![7; 4096]))])?;
        ensure!(
            plain.len() < 4096,
            "Buffers were not compressed: {}",
            plain.len()
        );
        Ok(())
    }
}
