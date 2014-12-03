#![crate_type = "lib"]
#![feature(unsafe_destructor)]

struct S<'a> {
    p: &'a mut uint,
}

impl<'a> S<'a> {
    fn new(p: &'a mut uint) -> S<'a> { S { p: p } }
}

#[unsafe_destructor]
impl<'a> Drop for S<'a> {
    fn drop(&mut self) {
        *self.p = 3;
    }
}

fn crust_init() -> (uint,) { (2,) }