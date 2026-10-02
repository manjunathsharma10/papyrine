use std::ops::Range;

/// An operand value. Integral numbers are always `Int` (so `4.`, `4.0` and `4`
/// parse identically); `Real` is therefore never integral below 9e18.
#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// Decoded string bytes (literal and hex forms are not distinguished).
    Str(Vec<u8>),
    /// Decoded name bytes, without the leading slash.
    Name(Vec<u8>),
    Array(Vec<Object>),
    /// Key order is preserved; duplicate keys are kept.
    Dict(Vec<(Vec<u8>, Object)>),
}

impl Object {
    pub fn name(s: &str) -> Object {
        Object::Name(s.as_bytes().to_vec())
    }

    pub fn string(s: impl AsRef<[u8]>) -> Object {
        Object::Str(s.as_ref().to_vec())
    }

    /// Int or Real as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Object::Int(i) => Some(i as f64),
            Object::Real(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Object::Int(i) => Some(i),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_str_bytes(&self) -> Option<&[u8]> {
        match self {
            Object::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl From<i64> for Object {
    fn from(v: i64) -> Self {
        Object::Int(v)
    }
}

impl From<f64> for Object {
    /// Integral values become `Int`, matching what the parser produces.
    fn from(v: f64) -> Self {
        crate::lexer::number_from_f64(v)
    }
}

impl From<bool> for Object {
    fn from(v: bool) -> Self {
        Object::Bool(v)
    }
}

/// One operator with its operands.
#[derive(Clone, Debug)]
pub struct Op {
    pub operator: Vec<u8>,
    pub operands: Vec<Object>,
    /// Raw image bytes for `BI`; its `operands` hold one `Dict` of the inline
    /// image parameters (abbreviated keys are kept as written).
    pub inline_data: Option<Vec<u8>>,
    /// Byte range in the parsed input, from the first operand to the end of the
    /// operator (for `BI`, through `EI`). Empty (`0..0`) for built ops.
    pub span: Range<usize>,
}

impl Op {
    pub fn new(operator: &str, operands: Vec<Object>) -> Op {
        Op {
            operator: operator.as_bytes().to_vec(),
            operands,
            inline_data: None,
            span: 0..0,
        }
    }

    pub fn is(&self, operator: &str) -> bool {
        self.operator == operator.as_bytes()
    }

    pub fn operator_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.operator)
    }

    /// Equality that ignores `span`.
    pub fn same_content(&self, other: &Op) -> bool {
        self.operator == other.operator
            && self.operands == other.operands
            && self.inline_data == other.inline_data
    }
}

pub fn ops_equal(a: &[Op], b: &[Op]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same_content(y))
}
