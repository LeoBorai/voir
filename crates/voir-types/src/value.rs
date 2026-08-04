use std::cmp::Ordering;
use std::sync::Arc;

use crate::error::VoirError;
use crate::schema::DataType;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Boolean(bool),
    Int64(i64),
    Float64(f64),
    Utf8(Arc<str>),
    Date(i32),
    Timestamp(i64),
}

impl Value {
    pub fn utf8(s: impl Into<Arc<str>>) -> Self {
        Value::Utf8(s.into())
    }

    #[must_use]
    pub fn data_type(&self) -> DataType {
        match self {
            Value::Null => DataType::Null,
            Value::Boolean(_) => DataType::Boolean,
            Value::Int64(_) => DataType::Int64,
            Value::Float64(_) => DataType::Float64,
            Value::Utf8(_) => DataType::Utf8,
            Value::Date(_) => DataType::Date,
            Value::Timestamp(_) => DataType::Timestamp,
        }
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    fn as_bool_or_null(&self) -> Result<Option<bool>, VoirError> {
        match self {
            Value::Null => Ok(None),
            Value::Boolean(b) => Ok(Some(*b)),
            other => Err(VoirError::Type(format!(
                "expected boolean, found {:?}",
                other.data_type()
            ))),
        }
    }

    // Only used for cross-type (Int64 vs Float64) comparison, where some
    // precision loss above 2^53 is an accepted v1 limitation (see plan.md
    // section 12 on Decimal being a post-v1 addition).
    #[allow(clippy::cast_precision_loss)]
    fn as_numeric(&self) -> Option<f64> {
        match self {
            Value::Int64(v) => Some(*v as f64),
            Value::Float64(v) => Some(*v),
            _ => None,
        }
    }

    /// # Errors
    ///
    /// Returns an error if `self` is not a boolean or `Null`.
    pub fn sql_not(&self) -> Result<Value, VoirError> {
        match self.as_bool_or_null()? {
            None => Ok(Value::Null),
            Some(b) => Ok(Value::Boolean(!b)),
        }
    }

    /// # Errors
    ///
    /// Returns an error if `self` is not `Null`, `Int64`, or `Float64`.
    pub fn sql_negate(&self) -> Result<Value, VoirError> {
        match self {
            Value::Null => Ok(Value::Null),
            Value::Int64(v) => Ok(Value::Int64(-v)),
            Value::Float64(v) => Ok(Value::Float64(-v)),
            other => Err(VoirError::Type(format!(
                "cannot negate {:?}",
                other.data_type()
            ))),
        }
    }

    // NULL AND false -> false; NULL AND true -> NULL; NULL AND NULL -> NULL.
    /// # Errors
    ///
    /// Returns an error if either operand is not a boolean or `Null`.
    pub fn sql_and(&self, other: &Value) -> Result<Value, VoirError> {
        match (self.as_bool_or_null()?, other.as_bool_or_null()?) {
            (Some(false), _) | (_, Some(false)) => Ok(Value::Boolean(false)),
            (Some(true), Some(true)) => Ok(Value::Boolean(true)),
            _ => Ok(Value::Null),
        }
    }

    // NULL OR true -> true; NULL OR false -> NULL; NULL OR NULL -> NULL.
    /// # Errors
    ///
    /// Returns an error if either operand is not a boolean or `Null`.
    pub fn sql_or(&self, other: &Value) -> Result<Value, VoirError> {
        match (self.as_bool_or_null()?, other.as_bool_or_null()?) {
            (Some(true), _) | (_, Some(true)) => Ok(Value::Boolean(true)),
            (Some(false), Some(false)) => Ok(Value::Boolean(false)),
            _ => Ok(Value::Null),
        }
    }

    /// # Errors
    ///
    /// Returns an error if the operands are of incomparable types.
    pub fn sql_eq(&self, other: &Value) -> Result<Value, VoirError> {
        self.compare(other, |o| o == Ordering::Equal)
    }

    /// # Errors
    ///
    /// Returns an error if the operands are of incomparable types.
    pub fn sql_not_eq(&self, other: &Value) -> Result<Value, VoirError> {
        self.compare(other, |o| o != Ordering::Equal)
    }

    /// # Errors
    ///
    /// Returns an error if the operands are of incomparable types.
    pub fn sql_lt(&self, other: &Value) -> Result<Value, VoirError> {
        self.compare(other, |o| o == Ordering::Less)
    }

    /// # Errors
    ///
    /// Returns an error if the operands are of incomparable types.
    pub fn sql_lt_eq(&self, other: &Value) -> Result<Value, VoirError> {
        self.compare(other, |o| o != Ordering::Greater)
    }

    /// # Errors
    ///
    /// Returns an error if the operands are of incomparable types.
    pub fn sql_gt(&self, other: &Value) -> Result<Value, VoirError> {
        self.compare(other, |o| o == Ordering::Greater)
    }

    /// # Errors
    ///
    /// Returns an error if the operands are of incomparable types.
    pub fn sql_gt_eq(&self, other: &Value) -> Result<Value, VoirError> {
        self.compare(other, |o| o != Ordering::Less)
    }

    // NULL = NULL -> NULL, not true. Comparisons never see through Null;
    // use IS NOT DISTINCT FROM (not yet implemented) for null-safe equality.
    fn compare(
        &self,
        other: &Value,
        matches_ordering: impl Fn(Ordering) -> bool,
    ) -> Result<Value, VoirError> {
        if self.is_null() || other.is_null() {
            return Ok(Value::Null);
        }
        let ordering = match (self, other) {
            (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b),
            (Value::Utf8(a), Value::Utf8(b)) => a.as_ref().cmp(b.as_ref()),
            (Value::Date(a), Value::Date(b)) => a.cmp(b),
            (Value::Timestamp(a), Value::Timestamp(b)) => a.cmp(b),
            (a, b) => match (a.as_numeric(), b.as_numeric()) {
                (Some(x), Some(y)) => x
                    .partial_cmp(&y)
                    .ok_or_else(|| VoirError::Type("comparison with NaN".into()))?,
                _ => {
                    return Err(VoirError::Type(format!(
                        "cannot compare {:?} with {:?}",
                        a.data_type(),
                        b.data_type()
                    )));
                }
            },
        };
        Ok(Value::Boolean(matches_ordering(ordering)))
    }

    /// # Errors
    ///
    /// Returns an error on integer overflow or a type mismatch.
    pub fn sql_add(&self, other: &Value) -> Result<Value, VoirError> {
        self.checked_arith(other, "+", i64::checked_add, |a, b| a + b)
    }

    /// # Errors
    ///
    /// Returns an error on integer overflow or a type mismatch.
    pub fn sql_sub(&self, other: &Value) -> Result<Value, VoirError> {
        self.checked_arith(other, "-", i64::checked_sub, |a, b| a - b)
    }

    /// # Errors
    ///
    /// Returns an error on integer overflow or a type mismatch.
    pub fn sql_mul(&self, other: &Value) -> Result<Value, VoirError> {
        self.checked_arith(other, "*", i64::checked_mul, |a, b| a * b)
    }

    /// # Errors
    ///
    /// Returns an error on division by zero or a type mismatch.
    pub fn sql_div(&self, other: &Value) -> Result<Value, VoirError> {
        if let (Value::Int64(a), Value::Int64(b)) = (self, other) {
            if *b == 0 {
                return Err(VoirError::Type("division by zero".into()));
            }
            return Ok(Value::Int64(a / b));
        }
        self.float_arith(other, "/", |a, b| a / b)
    }

    /// # Errors
    ///
    /// Returns an error on division by zero or a type mismatch.
    pub fn sql_rem(&self, other: &Value) -> Result<Value, VoirError> {
        if let (Value::Int64(a), Value::Int64(b)) = (self, other) {
            if *b == 0 {
                return Err(VoirError::Type("division by zero".into()));
            }
            return Ok(Value::Int64(a % b));
        }
        self.float_arith(other, "%", |a, b| a % b)
    }

    // Int64 + Int64 stays exact i64 arithmetic (overflow errors rather than
    // silently wrapping or losing precision through a float roundtrip);
    // anything else promotes to Float64.
    fn checked_arith(
        &self,
        other: &Value,
        op: &str,
        int_op: impl Fn(i64, i64) -> Option<i64>,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> Result<Value, VoirError> {
        if self.is_null() || other.is_null() {
            return Ok(Value::Null);
        }
        if let (Value::Int64(a), Value::Int64(b)) = (self, other) {
            return int_op(*a, *b)
                .map(Value::Int64)
                .ok_or_else(|| VoirError::Type(format!("integer overflow in '{op}'")));
        }
        self.float_arith(other, op, float_op)
    }

    fn float_arith(
        &self,
        other: &Value,
        op: &str,
        f: impl Fn(f64, f64) -> f64,
    ) -> Result<Value, VoirError> {
        if self.is_null() || other.is_null() {
            return Ok(Value::Null);
        }
        match (self.as_numeric(), other.as_numeric()) {
            (Some(a), Some(b)) => Ok(Value::Float64(f(a, b))),
            _ => Err(VoirError::Type(format!(
                "cannot apply '{op}' to {:?} and {:?}",
                self.data_type(),
                other.data_type()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: bool) -> Value {
        Value::Boolean(v)
    }
    const NULL: Value = Value::Null;

    #[test]
    fn and_truth_table() {
        assert_eq!(b(true).sql_and(&b(true)).unwrap(), b(true));
        assert_eq!(b(true).sql_and(&b(false)).unwrap(), b(false));
        assert_eq!(b(false).sql_and(&b(true)).unwrap(), b(false));
        assert_eq!(b(false).sql_and(&b(false)).unwrap(), b(false));
        assert_eq!(b(true).sql_and(&NULL).unwrap(), NULL);
        assert_eq!(NULL.sql_and(&b(true)).unwrap(), NULL);
        assert_eq!(b(false).sql_and(&NULL).unwrap(), b(false));
        assert_eq!(NULL.sql_and(&b(false)).unwrap(), b(false));
        assert_eq!(NULL.sql_and(&NULL).unwrap(), NULL);
    }

    #[test]
    fn or_truth_table() {
        assert_eq!(b(true).sql_or(&b(true)).unwrap(), b(true));
        assert_eq!(b(true).sql_or(&b(false)).unwrap(), b(true));
        assert_eq!(b(false).sql_or(&b(false)).unwrap(), b(false));
        assert_eq!(b(false).sql_or(&NULL).unwrap(), NULL);
        assert_eq!(NULL.sql_or(&b(false)).unwrap(), NULL);
        assert_eq!(b(true).sql_or(&NULL).unwrap(), b(true));
        assert_eq!(NULL.sql_or(&b(true)).unwrap(), b(true));
        assert_eq!(NULL.sql_or(&NULL).unwrap(), NULL);
    }

    #[test]
    fn not_truth_table() {
        assert_eq!(b(true).sql_not().unwrap(), b(false));
        assert_eq!(b(false).sql_not().unwrap(), b(true));
        assert_eq!(NULL.sql_not().unwrap(), NULL);
    }

    #[test]
    fn null_equality_is_unknown() {
        assert_eq!(NULL.sql_eq(&NULL).unwrap(), NULL);
        assert_eq!(Value::Int64(1).sql_eq(&NULL).unwrap(), NULL);
        assert_eq!(NULL.sql_eq(&Value::Int64(1)).unwrap(), NULL);
    }

    #[test]
    fn numeric_equality_and_ordering() {
        assert_eq!(Value::Int64(1).sql_eq(&Value::Int64(1)).unwrap(), b(true));
        assert_eq!(
            Value::Int64(1).sql_eq(&Value::Float64(1.0)).unwrap(),
            b(true)
        );
        assert_eq!(Value::Int64(1).sql_lt(&Value::Int64(2)).unwrap(), b(true));
        assert_eq!(
            Value::Int64(2).sql_gt_eq(&Value::Int64(2)).unwrap(),
            b(true)
        );
    }

    #[test]
    fn division_by_zero_errors() {
        assert!(Value::Int64(1).sql_div(&Value::Int64(0)).is_err());
    }

    #[test]
    fn integer_overflow_errors_instead_of_wrapping_or_losing_precision() {
        assert!(Value::Int64(i64::MAX).sql_add(&Value::Int64(1)).is_err());
        assert_eq!(
            Value::Int64(2).sql_mul(&Value::Int64(3)).unwrap(),
            Value::Int64(6)
        );
    }

    #[test]
    fn type_mismatch_errors_instead_of_coercing() {
        assert!(Value::utf8("abc").sql_add(&Value::Int64(1)).is_err());
    }
}
