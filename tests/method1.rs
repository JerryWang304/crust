#![crate_type = "lib"]

struct S {
    x: uint,
}

impl S {
    fn get(self) -> uint {
        self.x
    }
}

fn get_x(s: S) -> uint {
    s.get()
}
