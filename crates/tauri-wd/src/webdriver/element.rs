use std::collections::HashMap;

use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ElementRef {
    pub id: String,
    pub js_ref: String,
}

#[derive(Debug, Default)]
pub struct ElementStore {
    elements: HashMap<String, ElementRef>,
}

impl ElementStore {
    pub fn new() -> Self {
        Self {
            elements: HashMap::new(),
        }
    }

    pub fn store(&mut self) -> ElementRef {
        let id = Uuid::new_v4().to_string();
        self.register(&id).expect("generated UUID is valid")
    }

    /// Registers an element ID created by the in-page JSON clone algorithm.
    pub fn register(&mut self, id: &str) -> Option<ElementRef> {
        Uuid::parse_str(id).ok()?;
        if let Some(existing) = self.elements.get(id) {
            return Some(existing.clone());
        }
        let id_no_hyphens = id.replace('-', "");
        let js_ref = format!("__wd_el_{id_no_hyphens}");

        let elem_ref = ElementRef {
            id: id.to_owned(),
            js_ref,
        };

        self.elements.insert(id.to_owned(), elem_ref.clone());
        Some(elem_ref)
    }

    pub fn get(&self, id: &str) -> Option<&ElementRef> {
        self.elements.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_element() {
        let mut store = ElementStore::new();
        let elem = store.store();

        assert!(!elem.id.is_empty());
        assert!(elem.js_ref.starts_with("__wd_el_"));
        assert!(elem.js_ref.contains(&elem.id.replace('-', "")));
    }

    #[test]
    fn test_get_element() {
        let mut store = ElementStore::new();
        let elem = store.store();
        let id = elem.id.clone();

        let retrieved = store.get(&id).expect("element should exist");
        assert_eq!(retrieved.id, id);
    }

    #[test]
    fn test_js_ref_uses_id_without_hyphens() {
        let mut store = ElementStore::new();
        let elem1 = store.store();
        let elem2 = store.store();

        assert_eq!(
            elem1.js_ref,
            format!("__wd_el_{}", elem1.id.replace('-', ""))
        );
        assert_eq!(
            elem2.js_ref,
            format!("__wd_el_{}", elem2.id.replace('-', ""))
        );
    }
}
