use crate::{
    batch::{Link2, Single},
    operation::Oneshot,
};

/// Extension trait for adding helper methods to operations.
pub trait OperationExt {
    /// Link the operation to another.
    fn link_with<T>(self, another: T) -> Link2<Self, T>
    where
        Self: Oneshot + Sized,
        T: Oneshot,
    {
        Link2::new(self, another)
    }

    /// Convert the operation into a batch.
    fn into_batch(self) -> Single<Self>
    where
        Self: Oneshot + Sized,
    {
        Single::new(self)
    }
}

impl<O> OperationExt for O {}
