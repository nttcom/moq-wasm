import { useState } from 'react'

export function SecretReveal({ secret }: { secret: string }) {
  const [copied, setCopied] = useState(false)

  const copy = async () => {
    await navigator.clipboard.writeText(secret)
    setCopied(true)
  }

  return (
    <section className="flex flex-col gap-2 rounded-md border border-accent bg-wash p-4">
      <h3 className="text-sm font-semibold">app の secret</h3>
      <code className="rounded-md border border-line bg-white p-2 font-mono text-xs break-all">{secret}</code>
      <div>
        <button type="button" onClick={copy} className="rounded-md border border-line bg-white px-3 py-1.5 text-sm">
          {copied ? 'コピーしました' : 'コピー'}
        </button>
      </div>
      <p className="text-sm text-muted">
        この secret は、今しか表示されません。自分のサーバーで JWT
        に署名するときに使います。安全な場所に保管してください。漏れたときは作り直します。
      </p>
    </section>
  )
}
