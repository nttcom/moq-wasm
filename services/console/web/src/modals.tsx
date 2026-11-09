import { useState, type FormEvent, type ReactNode } from 'react'

import { api, describeError, formatUnixTime, type AppWithSecret, type ConsoleApp, type IssuedToken } from './api'
import { Checkbox, type TokenSummary } from './pages'
import { displayName } from './Sidebar'
import { Alert, Button, CopyButton, ErrorText, Modal } from './ui'

const maxNameChars = 64

export function CreateAppModal({
  onCreated,
  onClose
}: {
  onCreated: (result: AppWithSecret) => void
  onClose: () => void
}) {
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const trimmed = name.trim()

  const submit = async (event?: FormEvent) => {
    event?.preventDefault()
    setBusy(true)
    setError(null)
    try {
      onCreated(await api.createApp(trimmed))
    } catch (caught) {
      setError(describeError(caught))
      setBusy(false)
    }
  }

  return (
    <Modal
      title="app を作成"
      width="max-w-[440px]"
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            キャンセル
          </Button>
          <Button onClick={() => submit()} disabled={busy || trimmed === '' || [...trimmed].length > maxNameChars}>
            作成して secret を表示
          </Button>
        </>
      }
    >
      <form onSubmit={submit} className="flex flex-col gap-1.5">
        <label htmlFor="app-name" className="text-[13px] font-bold text-strong">
          app 名
        </label>
        <input
          id="app-name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          maxLength={maxNameChars}
          placeholder="live-demo"
          className="rounded-lg border border-line px-3 py-2 text-sm focus-visible:outline-2 focus-visible:outline-primary"
        />
        <span className="text-xs text-muted">表示用の名前です。1〜64 文字。後から変更できません。</span>
      </form>
      <ErrorText message={error} />
      <Alert tone="warning">作成直後に、署名用の secret が一度だけ表示されます。安全な場所に保存してください。</Alert>
    </Modal>
  )
}

type SecretModalProps = { app: ConsoleApp; secret: string; rotated: boolean; onClose: () => void }

export function SecretModal({ app, secret, rotated, onClose }: SecretModalProps) {
  const [saved, setSaved] = useState(false)
  return (
    <Modal
      title={rotated ? '新しい secret' : 'secret を保存してください'}
      subtitle={
        <>
          app <span className="font-mono">{displayName(app)}</span> の署名用 secret
        </>
      }
      width="max-w-[560px]"
      footer={
        <Button onClick={onClose} disabled={!saved}>
          閉じる
        </Button>
      }
    >
      <div className="flex items-start gap-3 rounded-lg border border-line bg-surface px-4 py-3.5">
        <code className="flex-1 font-mono text-sm break-all text-strong">{secret}</code>
        <CopyButton value={secret} />
      </div>
      <Alert tone="danger" title="この secret は今回だけ表示されます">
        閉じると二度と表示できません。紛失した場合は作り直しが必要で、その時点で発行済みのトークンはすべて使えなくなります。
      </Alert>
      <Checkbox checked={saved} onChange={setSaved} label="secret を安全な場所に保存しました" />
    </Modal>
  )
}

type ConfirmProps = { app: ConsoleApp; onDone: (result: AppWithSecret) => void; onClose: () => void }

export function RotateSecretModal({ app, onDone, onClose }: ConfirmProps) {
  return (
    <ConfirmModal
      title="secret を作り直しますか？"
      width="max-w-[480px]"
      confirmLabel="作り直す"
      onConfirm={async () => onDone(await api.rotateSecret(app.appId))}
      onClose={onClose}
    >
      <Alert tone="danger" title="すべてのトークンが使えなくなります">
        サーバーで署名したトークンも、コンソールで発行したトークンも使えなくなります。接続中のクライアントは、次の接続かトークンの更新で断られます。
      </Alert>
      <p className="text-sm text-body">
        新しい secret は、この後一度だけ表示されます。サーバーの設定を更新する準備ができてから実行してください。
      </p>
    </ConfirmModal>
  )
}

export function DeleteAppModal({
  app,
  onDeleted,
  onClose
}: {
  app: ConsoleApp
  onDeleted: () => void
  onClose: () => void
}) {
  return (
    <ConfirmModal
      title="app を削除しますか？"
      width="max-w-[460px]"
      confirmLabel="削除する"
      onConfirm={async () => {
        await api.deleteApp(app.appId)
        onDeleted()
      }}
      onClose={onClose}
    >
      <p className="text-sm leading-relaxed text-body">
        <span className="font-mono">{displayName(app)}</span> の secret
        と発行済みのトークンは使えなくなり、接続中のクライアントは次の接続かトークンの更新で断られます。この操作は元に戻せません。
      </p>
    </ConfirmModal>
  )
}

function ConfirmModal({
  title,
  width,
  confirmLabel,
  onConfirm,
  onClose,
  children
}: {
  title: string
  width: string
  confirmLabel: string
  onConfirm: () => Promise<void>
  onClose: () => void
  children: ReactNode
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const confirm = async () => {
    setBusy(true)
    setError(null)
    try {
      await onConfirm()
    } catch (caught) {
      setError(describeError(caught))
      setBusy(false)
    }
  }
  return (
    <Modal
      title={title}
      width={width}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            キャンセル
          </Button>
          <Button variant="danger" onClick={confirm} disabled={busy}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      {children}
      <ErrorText message={error} />
    </Modal>
  )
}

export function TokenModal({
  issued,
  summary,
  onClose
}: {
  issued: IssuedToken
  summary: TokenSummary
  onClose: () => void
}) {
  return (
    <Modal
      title="トークンを発行しました"
      width="max-w-[600px]"
      onClose={onClose}
      footer={<Button onClick={onClose}>閉じる</Button>}
    >
      <div className="flex items-start gap-3 rounded-lg border border-line bg-surface px-4 py-3.5">
        <code className="flex-1 font-mono text-[12.5px] break-all text-strong">{issued.token}</code>
        <CopyButton value={issued.token} />
      </div>
      <dl className="grid grid-cols-3 gap-3 text-[13px]">
        <Meta label="操作" value={summary.operations} />
        <Meta label="パス" value={summary.scope} mono />
        <Meta label="期限" value={formatUnixTime(issued.expiresAt)} />
      </dl>
      <Alert tone="warning" title="この画面を閉じると再表示できません">
        期限まで取り消すことはできません。取り消しが必要な場合は secret
        を作り直してください（すべてのトークンが使えなくなります）。
      </Alert>
    </Modal>
  )
}

function Meta({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-xs text-muted">{label}</dt>
      <dd className={`text-strong ${mono ? 'font-mono text-xs break-all' : ''}`}>{value}</dd>
    </div>
  )
}
