export type Diagnostic = {
  level: 'error' | 'warning'
  message: string
  rendered?: string
  line?: number
  column?: number
}

export type RunResult = {
  ok: boolean
  stdout: string
  result: string | null
  error: string | null
  diagnostics: Diagnostic[]
  elapsedMs: number
}

type LkWasmModule = {
  default: () => Promise<unknown>
  runLk: (source: string) => RunResult
}

let modulePromise: Promise<LkWasmModule> | undefined

export async function loadLkWasm(): Promise<LkWasmModule> {
  modulePromise ??= import('../wasm/pkg/lk_wasm.js')
    .then(async (mod) => {
      const wasm = mod as LkWasmModule
      await wasm.default()
      return wasm
    })
    .catch((error) => {
      modulePromise = undefined
      throw error
    })
  return modulePromise
}
