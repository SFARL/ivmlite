use crate::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Row(pub Vec<Value>);

impl Row {
    pub fn new(values: Vec<Value>) -> Self {
        Row(values)
    }

    pub fn get(&self, index: usize) -> &Value {
        &self.0[index]
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    #[test]
    fn row_exposes_values_by_index() {
        let r = Row::new(vec![Value::Int(7), Value::Null]);
        assert_eq!(r.len(), 2);
        assert_eq!(r.get(0), &Value::Int(7));
        assert_eq!(r.get(1), &Value::Null);
    }

    #[test]
    fn rows_sort_deterministically() {
        let a = Row::new(vec![Value::Int(1)]);
        let b = Row::new(vec![Value::Int(2)]);
        let mut v = vec![b.clone(), a.clone()];
        v.sort();
        assert_eq!(v, vec![a, b]);
    }
}
