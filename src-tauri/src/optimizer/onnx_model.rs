//! Static ONNX model inspection (no runtime, no GPU, no inference).
//!
//! Reads just enough of the protobuf wire format to learn what a model needs:
//! opset, operator histogram, initializer data types (quantized / float16 /
//! float32), dynamic input dimensions, external weight files, and whether it
//! looks like a generative LLM. The reader streams (`Read + Seek`) and skips
//! weight blobs without loading them, so a multi-GB model costs kilobytes of
//! memory. Every length is bounds-checked and nesting is capped, so a corrupt
//! or hostile file produces an error, never a panic or a huge allocation.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};

use serde::{Deserialize, Serialize};

const MAX_DEPTH: u32 = 8;
/// Largest single non-skipped field (a node, a name, a dim): 16 MiB.
const MAX_FIELD_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NODES: u32 = 2_000_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub file_size_bytes: u64,
    pub ir_version: Option<i64>,
    /// Version of the default (`""`/`ai.onnx`) operator set.
    pub opset: Option<i64>,
    pub producer: Option<String>,
    pub node_count: u32,
    /// Operator histogram, `domain::OpType`-free (default domain only keeps the bare name).
    pub operators: BTreeMap<String, u32>,
    pub initializer_count: u32,
    /// Initializer element types, by ONNX `TensorProto.DataType` name.
    pub initializer_types: BTreeMap<String, u32>,
    pub uses_external_data: bool,
    pub input_names: Vec<String>,
    /// A graph input has a symbolic or unknown dimension.
    pub dynamic_input_shapes: bool,
    pub quantized: bool,
    /// Weights are int4/NF4-style block-quantized (`MatMulNBits` and friends).
    pub block_quantized: bool,
    pub float16_weights: bool,
    /// Attention/KV-cache structure typical of a generative language model.
    pub genai_like: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Io(String),
    Truncated,
    NotOnnx(String),
    TooBig(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "could not read the model: {e}"),
            ParseError::Truncated => write!(f, "the file ends in the middle of a protobuf field (truncated or not an ONNX model)"),
            ParseError::NotOnnx(why) => write!(f, "not an ONNX model: {why}"),
            ParseError::TooBig(what) => write!(f, "model structure too large to inspect safely ({what})"),
        }
    }
}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            ParseError::Truncated
        } else {
            ParseError::Io(e.to_string())
        }
    }
}

type R<T> = Result<T, ParseError>;

struct Reader<S: Read + Seek> {
    s: S,
}

impl<S: Read + Seek> Reader<S> {
    fn pos(&mut self) -> R<u64> {
        Ok(self.s.stream_position()?)
    }
    fn byte(&mut self) -> R<u8> {
        let mut b = [0u8; 1];
        self.s.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn varint(&mut self) -> R<u64> {
        let mut v = 0u64;
        for shift in (0..70).step_by(7) {
            let b = self.byte()?;
            v |= ((b & 0x7F) as u64) << shift.min(63);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(ParseError::NotOnnx("varint longer than 10 bytes".into()))
    }
    fn tag(&mut self) -> R<(u32, u8)> {
        let t = self.varint()?;
        Ok(((t >> 3) as u32, (t & 7) as u8))
    }
    fn bytes(&mut self, len: u64) -> R<Vec<u8>> {
        if len > MAX_FIELD_BYTES {
            return Err(ParseError::TooBig("a single field"));
        }
        let mut buf = vec![0u8; len as usize];
        self.s.read_exact(&mut buf)?;
        Ok(buf)
    }
    fn skip(&mut self, len: u64, end: u64) -> R<()> {
        let pos = self.pos()?;
        if pos.checked_add(len).map(|p| p > end).unwrap_or(true) {
            return Err(ParseError::Truncated);
        }
        self.s.seek(SeekFrom::Current(len as i64))?;
        Ok(())
    }
    /// Skips a field of the given wire type (after its tag was read).
    fn skip_field(&mut self, wire: u8, end: u64) -> R<()> {
        match wire {
            0 => self.varint().map(|_| ()),
            1 => self.skip(8, end),
            2 => {
                let len = self.varint()?;
                self.skip(len, end)
            }
            5 => self.skip(4, end),
            w => Err(ParseError::NotOnnx(format!("unsupported wire type {w}"))),
        }
    }
    /// Reads the length prefix of a sub-message and returns its end offset.
    fn submessage_end(&mut self, parent_end: u64) -> R<u64> {
        let len = self.varint()?;
        let pos = self.pos()?;
        let end = pos.checked_add(len).ok_or(ParseError::Truncated)?;
        if end > parent_end {
            return Err(ParseError::Truncated);
        }
        Ok(end)
    }
}

fn datatype_name(code: u64) -> &'static str {
    match code {
        1 => "float32",
        2 => "uint8",
        3 => "int8",
        4 => "uint16",
        5 => "int16",
        6 => "int32",
        7 => "int64",
        8 => "string",
        9 => "bool",
        10 => "float16",
        11 => "float64",
        12 => "uint32",
        13 => "uint64",
        14 => "complex64",
        15 => "complex128",
        16 => "bfloat16",
        17 => "float8e4m3fn",
        18 => "float8e4m3fnuz",
        19 => "float8e5m2",
        20 => "float8e5m2fnuz",
        21 => "uint4",
        22 => "int4",
        _ => "other",
    }
}

/// Inspects the model in `stream`; `file_size` is recorded as-is.
pub fn inspect<S: Read + Seek>(stream: S, file_size: u64) -> Result<ModelProfile, ParseError> {
    let mut r = Reader { s: stream };
    let mut profile = ModelProfile { file_size_bytes: file_size, ..Default::default() };
    let end = file_size;
    let mut saw_graph = false;
    while r.pos()? < end {
        let (field, wire) = r.tag()?;
        match (field, wire) {
            (1, 0) => profile.ir_version = Some(r.varint()? as i64),
            (2, 2) => {
                let len = r.varint()?;
                profile.producer = Some(String::from_utf8_lossy(&r.bytes(len)?).into_owned());
            }
            (8, 2) => {
                let sub_end = r.submessage_end(end)?;
                let (domain, version) = read_opset(&mut r, sub_end)?;
                if domain.is_empty() || domain == "ai.onnx" {
                    profile.opset = Some(version);
                }
            }
            (7, 2) => {
                let sub_end = r.submessage_end(end)?;
                read_graph(&mut r, sub_end, &mut profile)?;
                saw_graph = true;
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    if !saw_graph {
        return Err(ParseError::NotOnnx("no graph found".into()));
    }
    finalize(&mut profile);
    Ok(profile)
}

fn read_opset<S: Read + Seek>(r: &mut Reader<S>, end: u64) -> R<(String, i64)> {
    let (mut domain, mut version) = (String::new(), 0i64);
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (1, 2) => {
                let len = r.varint()?;
                domain = String::from_utf8_lossy(&r.bytes(len)?).into_owned();
            }
            (2, 0) => version = r.varint()? as i64,
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok((domain, version))
}

fn read_graph<S: Read + Seek>(r: &mut Reader<S>, end: u64, p: &mut ModelProfile) -> R<()> {
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (1, 2) => {
                let sub_end = r.submessage_end(end)?;
                p.node_count += 1;
                if p.node_count > MAX_NODES {
                    return Err(ParseError::TooBig("more than 2,000,000 nodes"));
                }
                let (op, domain) = read_node(r, sub_end)?;
                let key = if domain.is_empty() || domain == "ai.onnx" { op } else { format!("{domain}::{op}") };
                *p.operators.entry(key).or_default() += 1;
            }
            (5, 2) => {
                let sub_end = r.submessage_end(end)?;
                p.initializer_count += 1;
                let (data_type, external) = read_tensor_header(r, sub_end)?;
                *p.initializer_types.entry(datatype_name(data_type).to_string()).or_default() += 1;
                p.uses_external_data |= external;
            }
            (11, 2) => {
                let sub_end = r.submessage_end(end)?;
                let (name, dynamic) = read_value_info(r, sub_end)?;
                p.input_names.push(name);
                p.dynamic_input_shapes |= dynamic;
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok(())
}

fn read_node<S: Read + Seek>(r: &mut Reader<S>, end: u64) -> R<(String, String)> {
    let (mut op, mut domain) = (String::new(), String::new());
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (4, 2) => {
                let len = r.varint()?;
                op = String::from_utf8_lossy(&r.bytes(len)?).into_owned();
            }
            (7, 2) => {
                let len = r.varint()?;
                domain = String::from_utf8_lossy(&r.bytes(len)?).into_owned();
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok((op, domain))
}

/// Returns `(data_type, has_external_data)`; weight bytes are seeked over.
fn read_tensor_header<S: Read + Seek>(r: &mut Reader<S>, end: u64) -> R<(u64, bool)> {
    let (mut data_type, mut external) = (0u64, false);
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (2, 0) => data_type = r.varint()?,
            (13, 2) => {
                external = true;
                let len = r.varint()?;
                r.skip(len, end)?;
            }
            (14, 0) => {
                // data_location: 1 = EXTERNAL
                if r.varint()? == 1 {
                    external = true;
                }
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok((data_type, external))
}

/// Returns `(name, has_dynamic_dim)`.
fn read_value_info<S: Read + Seek>(r: &mut Reader<S>, end: u64) -> R<(String, bool)> {
    let (mut name, mut dynamic) = (String::new(), false);
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (1, 2) => {
                let len = r.varint()?;
                name = String::from_utf8_lossy(&r.bytes(len)?).into_owned();
            }
            (2, 2) => {
                let sub_end = r.submessage_end(end)?;
                dynamic |= read_type(r, sub_end, 0)?;
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok((name, dynamic))
}

/// `ValueInfoProto.type` (TypeProto): `tensor_type` is field 1; other type
/// kinds (sequence, map, optional) have no static shape and are skipped.
fn read_type<S: Read + Seek>(r: &mut Reader<S>, end: u64, depth: u32) -> R<bool> {
    if depth > MAX_DEPTH {
        return Err(ParseError::TooBig("nesting"));
    }
    let mut dynamic = false;
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (1, 2) => {
                let sub_end = r.submessage_end(end)?;
                dynamic |= read_tensor_type(r, sub_end)?;
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok(dynamic)
}

/// `TypeProto.Tensor`: `elem_type` = 1, `shape` = 2.
fn read_tensor_type<S: Read + Seek>(r: &mut Reader<S>, end: u64) -> R<bool> {
    let mut dynamic = false;
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (2, 2) => {
                let sub_end = r.submessage_end(end)?;
                dynamic |= read_shape(r, sub_end)?;
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok(dynamic)
}

/// `TensorShapeProto`: repeated `dim` = 1; a dim without a positive
/// `dim_value` (symbolic `dim_param`, or unset) makes the shape dynamic.
fn read_shape<S: Read + Seek>(r: &mut Reader<S>, end: u64) -> R<bool> {
    let mut dynamic = false;
    while r.pos()? < end {
        let (f, w) = r.tag()?;
        match (f, w) {
            (1, 2) => {
                let sub_end = r.submessage_end(end)?;
                let mut fixed = false;
                while r.pos()? < sub_end {
                    let (df, dw) = r.tag()?;
                    match (df, dw) {
                        (1, 0) => fixed = r.varint()? as i64 > 0,
                        (_, dw) => r.skip_field(dw, sub_end)?,
                    }
                }
                dynamic |= !fixed;
            }
            (_, w) => r.skip_field(w, end)?,
        }
    }
    Ok(dynamic)
}

fn finalize(p: &mut ModelProfile) {
    let has = |names: &[&str]| p.operators.keys().any(|k| names.iter().any(|n| k == n || k.ends_with(&format!("::{n}"))));
    p.quantized = has(&["QuantizeLinear", "DequantizeLinear", "QLinearConv", "QLinearMatMul", "ConvInteger", "MatMulInteger", "DynamicQuantizeLinear"])
        || p.initializer_types.get("int8").copied().unwrap_or(0) + p.initializer_types.get("uint8").copied().unwrap_or(0) > p.initializer_count / 2 && p.initializer_count > 0;
    p.block_quantized = has(&["MatMulNBits", "GatherBlockQuantized"]) || p.initializer_types.contains_key("int4") || p.initializer_types.contains_key("uint4");
    p.float16_weights = p.initializer_types.get("float16").copied().unwrap_or(0) > p.initializer_count / 2 && p.initializer_count > 0;
    let kv_cache_inputs = p.input_names.iter().any(|n| n.contains("past_key_values") || n.starts_with("past_key") || n.contains("kv_cache"));
    p.genai_like = kv_cache_inputs || has(&["GroupQueryAttention", "MultiHeadAttention", "SkipSimplifiedLayerNormalization", "RotaryEmbedding"]) || p.operators.contains_key("com.microsoft::GroupQueryAttention");
}

/// Convenience: inspect a file on disk.
pub fn inspect_file(path: &std::path::Path) -> Result<ModelProfile, ParseError> {
    let ext_ok = path.extension().map(|e| e.eq_ignore_ascii_case("onnx") || e.eq_ignore_ascii_case("ort")).unwrap_or(false);
    if !ext_ok {
        return Err(ParseError::NotOnnx("expected a .onnx file".into()));
    }
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    inspect(std::io::BufReader::with_capacity(64 * 1024, file), size)
}

#[cfg(test)]
pub(crate) mod testing {
    //! A tiny protobuf *writer* so tests can build real ONNX bytes.
    pub fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    pub fn field_varint(field: u32, v: u64, out: &mut Vec<u8>) {
        varint(((field as u64) << 3) | 0, out);
        varint(v, out);
    }
    pub fn field_bytes(field: u32, body: &[u8], out: &mut Vec<u8>) {
        varint(((field as u64) << 3) | 2, out);
        varint(body.len() as u64, out);
        out.extend_from_slice(body);
    }
    pub fn node(op: &str) -> Vec<u8> {
        let mut n = Vec::new();
        field_bytes(4, op.as_bytes(), &mut n);
        n
    }
    pub fn tensor(data_type: u64, raw: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        field_varint(2, data_type, &mut t);
        field_bytes(9, raw, &mut t);
        t
    }
    /// A graph input with the given dims (`None` = symbolic).
    pub fn input(name: &str, dims: &[Option<i64>]) -> Vec<u8> {
        let mut shape = Vec::new();
        for d in dims {
            let mut dim = Vec::new();
            match d {
                Some(v) => field_varint(1, *v as u64, &mut dim),
                None => field_bytes(2, b"batch", &mut dim),
            }
            field_bytes(1, &dim, &mut shape);
        }
        let mut tensor_type = Vec::new();
        field_varint(1, 1, &mut tensor_type);
        field_bytes(2, &shape, &mut tensor_type);
        let mut ty = Vec::new();
        field_bytes(1, &tensor_type, &mut ty);
        let mut vi = Vec::new();
        field_bytes(1, name.as_bytes(), &mut vi);
        field_bytes(2, &ty, &mut vi);
        vi
    }
    pub fn model(opset: u64, nodes: &[Vec<u8>], tensors: &[Vec<u8>], inputs: &[Vec<u8>]) -> Vec<u8> {
        let mut graph = Vec::new();
        for n in nodes {
            field_bytes(1, n, &mut graph);
        }
        for t in tensors {
            field_bytes(5, t, &mut graph);
        }
        for i in inputs {
            field_bytes(11, i, &mut graph);
        }
        let mut m = Vec::new();
        field_varint(1, 8, &mut m);
        field_bytes(2, b"pytorch", &mut m);
        let mut opset_msg = Vec::new();
        field_varint(2, opset, &mut opset_msg);
        field_bytes(8, &opset_msg, &mut m);
        field_bytes(7, &graph, &mut m);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::io::Cursor;

    fn run(bytes: Vec<u8>) -> Result<ModelProfile, ParseError> {
        let len = bytes.len() as u64;
        inspect(Cursor::new(bytes), len)
    }

    #[test]
    fn reads_opset_operators_and_input_shapes() {
        let bytes = model(17, &[node("Conv"), node("Relu"), node("Conv")], &[tensor(1, &[0; 16])], &[input("images", &[Some(1), Some(3), Some(224), Some(224)])]);
        let p = run(bytes).unwrap();
        assert_eq!(p.opset, Some(17));
        assert_eq!(p.ir_version, Some(8));
        assert_eq!(p.producer.as_deref(), Some("pytorch"));
        assert_eq!(p.node_count, 3);
        assert_eq!(p.operators["Conv"], 2);
        assert_eq!(p.initializer_types["float32"], 1);
        assert!(!p.dynamic_input_shapes && !p.quantized && !p.genai_like);
        assert_eq!(p.input_names, vec!["images"]);
    }

    #[test]
    fn symbolic_dimensions_are_reported_as_dynamic_shapes() {
        let p = run(model(17, &[node("Gemm")], &[], &[input("x", &[None, Some(128)])])).unwrap();
        assert!(p.dynamic_input_shapes);
    }

    #[test]
    fn qdq_and_int8_models_are_recognised_as_quantized() {
        let p = run(model(13, &[node("QuantizeLinear"), node("DequantizeLinear")], &[tensor(3, &[0; 4])], &[])).unwrap();
        assert!(p.quantized);
        let qlinear = run(model(13, &[node("QLinearConv")], &[], &[])).unwrap();
        assert!(qlinear.quantized);
        let by_weights = run(model(13, &[node("Conv")], &[tensor(3, &[0; 4]), tensor(3, &[0; 4]), tensor(1, &[0; 4])], &[])).unwrap();
        assert!(by_weights.quantized, "mostly-int8 initializers count as quantized");
    }

    #[test]
    fn llm_structure_is_detected_from_kv_cache_inputs_and_attention_ops() {
        let by_input = run(model(17, &[node("MatMul")], &[], &[input("past_key_values.0.key", &[Some(1), Some(8), None, Some(64)])])).unwrap();
        assert!(by_input.genai_like);
        let by_op = run(model(17, &[node("GroupQueryAttention")], &[], &[])).unwrap();
        assert!(by_op.genai_like);
        let nbits = run(model(17, &[node("MatMulNBits")], &[tensor(22, &[0; 4])], &[])).unwrap();
        assert!(nbits.block_quantized);
    }

    #[test]
    fn float16_weight_models_are_flagged() {
        let p = run(model(17, &[node("Conv")], &[tensor(10, &[0; 4]), tensor(10, &[0; 4])], &[])).unwrap();
        assert!(p.float16_weights);
    }

    #[test]
    fn external_weight_files_are_noticed_without_reading_them() {
        let mut t = tensor(1, &[]);
        let mut entry = Vec::new();
        field_bytes(1, b"location", &mut entry);
        field_bytes(2, b"weights.bin", &mut entry);
        field_bytes(13, &entry, &mut t);
        let p = run(model(17, &[node("Conv")], &[t], &[])).unwrap();
        assert!(p.uses_external_data);
    }

    #[test]
    fn huge_weight_blobs_are_skipped_not_loaded() {
        // 40 MB of weights: far above the per-field read cap, so the reader must seek past it.
        let blob = vec![7u8; 40 * 1024 * 1024];
        let p = run(model(17, &[node("Conv")], &[tensor(1, &blob)], &[])).unwrap();
        assert_eq!(p.initializer_count, 1);
        assert!(p.file_size_bytes > 40 * 1024 * 1024);
    }

    #[test]
    fn garbage_truncation_and_lies_about_length_are_errors_not_panics() {
        assert!(matches!(run(vec![]), Err(ParseError::NotOnnx(_))));
        assert!(run(vec![0xFF; 64]).is_err());
        assert!(run(b"MZ\x90\x00 this is an exe".to_vec()).is_err());
        let good = model(17, &[node("Conv")], &[], &[]);
        for cut in [1usize, 5, good.len() / 2, good.len() - 1] {
            assert!(run(good[..cut].to_vec()).is_err(), "cut at {cut}");
        }
        // A length prefix larger than the file.
        let mut lie = Vec::new();
        field_varint(1, 8, &mut lie);
        varint((7 << 3) | 2, &mut lie);
        varint(u64::MAX >> 1, &mut lie);
        assert!(run(lie).is_err());
        // Over-long varint.
        assert!(run(vec![0x80; 20]).is_err());
    }

    #[test]
    fn only_onnx_files_are_opened() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("a.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(matches!(inspect_file(&txt), Err(ParseError::NotOnnx(_))));
        let onnx = dir.path().join("m.onnx");
        std::fs::write(&onnx, model(17, &[node("Relu")], &[], &[])).unwrap();
        assert_eq!(inspect_file(&onnx).unwrap().operators["Relu"], 1);
    }
}
