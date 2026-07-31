pub trait DynamicCall {
    fn invoke(&self) -> i32;
}

macro_rules! generated_call {
    ($call:expr) => {
        $call
    };
}

pub fn seed(target: &dyn DynamicCall) -> i32 {
    generated_call!(target.invoke())
}

pub fn caller(target: &dyn DynamicCall) -> i32 {
    seed(target)
}
