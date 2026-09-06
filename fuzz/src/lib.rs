//! Shared helpers for the fuzz targets in `fuzz_targets/`.

use arbitrary::Unstructured;
use serde_json::{Map, Value};

/// A bounded-depth arbitrary JSON value, for targets that want to fuzz a
/// parser's handling of `serde_json::Value` shapes (nesting, key names,
/// number/string edge cases) rather than raw bytes.
pub fn arbitrary_value(u: &mut Unstructured, depth: u8) -> arbitrary::Result<Value> {
    if depth == 0 {
        return arbitrary_scalar(u);
    }
    Ok(match u.int_in_range(0..=5)? {
        0..=2 => arbitrary_scalar(u)?,
        3 => {
            let len = u.int_in_range(0..=4)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(arbitrary_value(u, depth - 1)?);
            }
            Value::Array(items)
        }
        _ => {
            let len = u.int_in_range(0..=4)?;
            let mut map = Map::with_capacity(len);
            for _ in 0..len {
                let key: String = u.arbitrary()?;
                map.insert(key, arbitrary_value(u, depth - 1)?);
            }
            Value::Object(map)
        }
    })
}

fn arbitrary_scalar(u: &mut Unstructured) -> arbitrary::Result<Value> {
    Ok(match u.int_in_range(0..=4)? {
        0 => Value::Null,
        1 => Value::Bool(u.arbitrary()?),
        2 => Value::from(u.arbitrary::<i64>()?),
        3 => {
            // `unwrap_or(0.0)` rather than propagating: a NaN/inf f64 is a
            // valid `arbitrary` draw but not valid JSON, and skipping the
            // whole value on that draw would bias the corpus away from
            // numbers instead of just clamping this one.
            let f = u.arbitrary::<f64>()?;
            serde_json::Number::from_f64(f).map_or(Value::from(0), Value::Number)
        }
        _ => Value::String(u.arbitrary()?),
    })
}
