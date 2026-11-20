export class FakePictureElement extends EventTarget {
  readonly dataset: Record<string, string> = {}
  playCalls = 0

  setAttribute(): void {}

  remove(): void {}

  play(): Promise<void> {
    this.playCalls += 1
    return Promise.resolve()
  }
}

export class FakePictureContainer {
  prepend(): void {}
}

export function installFakePictureDocument(): () => void {
  const globals = globalThis as Record<string, unknown>
  globals.document = { createElement: () => new FakePictureElement() }
  return () => {
    delete globals.document
  }
}
