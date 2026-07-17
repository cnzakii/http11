//! Python extension module for `h11r`.

use pyo3::prelude::*;
use pyo3::types::PyModule;

mod api;

#[pymodule(gil_used = true)]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    api::register(module)
}
