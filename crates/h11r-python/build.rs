fn main() {
    // Exposes pyo3's interpreter cfgs, such as `Py_GIL_DISABLED`.
    pyo3_build_config::use_pyo3_cfgs();
}
