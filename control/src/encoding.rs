use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const LEVEL: i32 = 6;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Default, Deserialize)]
struct Request {
    #[serde(default)]
    encoding: Option<String>,
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

pub fn encode(raw: &str, document: Value) -> Result<Value> {
    let request: Request = serde_json::from_str(raw).unwrap_or_default();
    match request.encoding.as_deref() {
        None => Ok(document),
        Some("zstd") => {
            let packed = zstd::bulk::compress(&serde_json::to_vec(&document)?, LEVEL)?;
            Ok(json!({"encoding":"zstd","data":base64(&packed)}))
        }
        Some(other) => bail!("Unknown encoding {other}."),
    }
}

#[cfg(test)]
mod tests {
    use super::{base64, encode};
    use anyhow::{Context, Result, ensure};
    use serde_json::json;

    fn decode(text: &str) -> Vec<u8> {
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
    fn documents_round_trip_through_zstd() -> Result<()> {
        let document = json!({"start": 1, "series": [{"median": [1, null, 3]}]});
        ensure!(encode("{}", document.clone())? == document);
        let packed = encode(r#"{"encoding":"zstd"}"#, document.clone())?;
        ensure!(packed["encoding"] == "zstd");
        let data = packed["data"].as_str().context("Missing data.")?;
        let plain = zstd::bulk::decompress(&decode(data), 1 << 20)?;
        ensure!(serde_json::from_slice::<serde_json::Value>(&plain)? == document);
        ensure!(encode(r#"{"encoding":"brotli"}"#, document).is_err());
        Ok(())
    }
}
