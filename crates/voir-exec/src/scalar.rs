use voir_plan::ScalarFunc;
use voir_types::{DataType, Value, VoirError};

/// Evaluates a scalar function against already-evaluated argument values.
/// Arity was checked at bind time, so `args` is trusted to match `func`.
pub fn eval_scalar(func: ScalarFunc, args: &[Value]) -> Result<Value, VoirError> {
    match func {
        ScalarFunc::Upper => text_op(&args[0], str::to_uppercase),
        ScalarFunc::Lower => text_op(&args[0], str::to_lowercase),
        ScalarFunc::Trim => text_op(&args[0], |s| s.trim().to_string()),
        ScalarFunc::Length => match &args[0] {
            Value::Null => Ok(Value::Null),
            #[allow(clippy::cast_possible_wrap)]
            Value::Utf8(s) => Ok(Value::Int64(s.chars().count() as i64)),
            other => Err(type_err("length", other)),
        },
        ScalarFunc::Concat => Ok(Value::utf8(args.iter().map(stringify).collect::<String>())),
        ScalarFunc::Abs => numeric_unary(&args[0], f64::abs, i64::abs),
        ScalarFunc::Round => {
            let ndigits = match args.get(1) {
                Some(v) if v.is_null() => return Ok(Value::Null),
                #[allow(clippy::cast_possible_truncation)]
                Some(v) => v.as_f64()? as i32,
                None => 0,
            };
            eval_round(&args[0], ndigits)
        }
        ScalarFunc::Ceil => numeric_unary_f(&args[0], f64::ceil),
        ScalarFunc::Floor => numeric_unary_f(&args[0], f64::floor),
        ScalarFunc::Power => numeric_binary_f(&args[0], &args[1], f64::powf),
        ScalarFunc::Sqrt => numeric_unary_f(&args[0], f64::sqrt),
        ScalarFunc::Mod => numeric_binary_f(&args[0], &args[1], |a, b| a % b),
        ScalarFunc::Coalesce => Ok(args
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null)),
        ScalarFunc::JsonExtract => Ok(json_extract(args)),
        ScalarFunc::NullIf => {
            if args[0] == args[1] {
                Ok(Value::Null)
            } else {
                Ok(args[0].clone())
            }
        }
    }
}

fn text_op(v: &Value, f: impl Fn(&str) -> String) -> Result<Value, VoirError> {
    match v {
        Value::Null => Ok(Value::Null),
        Value::Utf8(s) => Ok(Value::utf8(f(s))),
        other => Err(type_err("string function", other)),
    }
}

fn numeric_unary(
    v: &Value,
    float_fn: impl Fn(f64) -> f64,
    int_fn: impl Fn(i64) -> i64,
) -> Result<Value, VoirError> {
    match v {
        Value::Null => Ok(Value::Null),
        Value::Int64(n) => Ok(Value::Int64(int_fn(*n))),
        Value::Float64(n) => Ok(Value::Float64(float_fn(*n))),
        other => Err(type_err("numeric function", other)),
    }
}

fn numeric_unary_f(v: &Value, f: impl Fn(f64) -> f64) -> Result<Value, VoirError> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    Ok(Value::Float64(f(v.as_f64()?)))
}

fn numeric_binary_f(a: &Value, b: &Value, f: impl Fn(f64, f64) -> f64) -> Result<Value, VoirError> {
    if a.is_null() || b.is_null() {
        return Ok(Value::Null);
    }
    Ok(Value::Float64(f(a.as_f64()?, b.as_f64()?)))
}

fn eval_round(v: &Value, ndigits: i32) -> Result<Value, VoirError> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let x = v.as_f64()?;
    let factor = 10f64.powi(ndigits);
    Ok(Value::Float64((x * factor).round() / factor))
}

fn stringify(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Utf8(s) => s.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Int64(n) | Value::Timestamp(n) => n.to_string(),
        Value::Float64(n) => n.to_string(),
        Value::Date(n) => n.to_string(),
    }
}

fn type_err(op: &str, v: &Value) -> VoirError {
    VoirError::Type(format!(
        "{op}: unexpected argument type {:?}",
        v.data_type()
    ))
}

/// `json_extract(text, key, type)`: `NULL` for non-objects, missing keys and
/// JSON `null`; nested objects/arrays come back as their JSON text.
fn json_extract(args: &[Value]) -> Value {
    let (Value::Utf8(text), Value::Utf8(key)) = (&args[0], &args[1]) else {
        return Value::Null;
    };
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(text) else {
        return Value::Null;
    };
    let target = match &args[2] {
        Value::Utf8(t) => match &**t {
            "Int64" => DataType::Int64,
            "Float64" => DataType::Float64,
            "Boolean" => DataType::Boolean,
            _ => DataType::Utf8,
        },
        _ => DataType::Utf8,
    };
    match obj.get(&**key) {
        None | Some(serde_json::Value::Null) => Value::Null,
        Some(serde_json::Value::String(s)) => Value::utf8(s.as_str()),
        Some(serde_json::Value::Bool(b)) if target == DataType::Boolean => Value::Boolean(*b),
        Some(serde_json::Value::Number(n)) => match target {
            DataType::Int64 => n.as_i64().map_or(Value::Null, Value::Int64),
            DataType::Float64 => n.as_f64().map_or(Value::Null, Value::Float64),
            _ => Value::utf8(n.to_string()),
        },
        Some(other) => Value::utf8(other.to_string()),
    }
}
