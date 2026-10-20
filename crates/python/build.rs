fn main() {
    // macOS leaves the Python symbols of an extension module for the interpreter to resolve at load time.
    pyo3_build_config::add_extension_module_link_args();
}
