use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
enum LazyValueInner<T, D> {
    Value(T),
    Fun(Box<dyn FnOnce(&mut D) -> T>),
    #[default]
    None,
}

pub struct LazyValue<T, D> {
    value: Rc<RefCell<LazyValueInner<T, D>>>,
}

impl<T, D> Clone for LazyValue<T, D> {
    fn clone(&self) -> Self {
        Self {
            value: Rc::clone(&self.value),
        }
    }
}

impl<T: std::fmt::Debug, D> std::fmt::Debug for LazyValue<T, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = self.try_get();
        f.debug_struct("LazyValue").field("value", &v).finish()
    }
}

impl<T, D> LazyValue<T, D> {
    pub fn make<F>(fun: F) -> Self
    where
        F: FnOnce(&mut D) -> T + 'static,
    {
        Self {
            value: Rc::new(RefCell::new(LazyValueInner::Fun(Box::new(fun)))),
        }
    }

    fn get_impl(&self) -> std::cell::Ref<'_, T> {
        use std::cell::Ref;

        let inner = self.value.borrow();
        Ref::map(inner, |inner| {
            let LazyValueInner::Value(value) = inner else {
                panic!("invalid state");
            };
            value
        })
    }

    /// Returns the value when it already has been "computed"
    pub fn try_get(&self) -> Option<std::cell::Ref<'_, T>> {
        let inner = self.value.borrow();

        match &*inner {
            LazyValueInner::Value(_) => {}
            LazyValueInner::Fun(_) => return None,
            LazyValueInner::None => panic!("invalid state"),
        }

        Some(self.get_impl())
    }

    pub fn get(&self, data: &mut D) -> std::cell::Ref<'_, T> {
        let v = self.value.borrow();

        if let LazyValueInner::Fun(_) = &*v {
            std::mem::drop(v);

            let LazyValueInner::Fun(fun) = self.value.take() else {
                panic!("invalid state");
            };

            let data = fun(data);
            self.value.replace(LazyValueInner::Value(data));
        };

        self.get_impl()
    }
}
