use crate::{Core, CoreExtract};

pub fn into_system<F, T: CoreExtract>(f: F) -> impl Fn(&mut Core)
where
    F: Fn(T),
{
    move |core: &mut Core| {
        let args = T::extract(core);
        f(args);
    }
}
