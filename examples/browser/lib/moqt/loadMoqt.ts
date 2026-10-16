type MoqtModule = typeof import('../../pkg/moqt')

let loading: Promise<MoqtModule> | null = null

/// Loads and instantiates the wasm package once per page. The generated `init`
/// only checks for an existing instance before its first `await`, so two
/// overlapping calls instantiate the module twice, and the glue code then routes
/// every object and callback the first instance created to the second instance.
export function loadMoqt(): Promise<MoqtModule> {
  loading ??= import('../../pkg/moqt')
    .then(async (moqt) => {
      await moqt.default()
      return moqt
    })
    .catch((error) => {
      loading = null
      throw error
    })
  return loading
}
