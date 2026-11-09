import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from 'react'

import { Icon, type IconName } from './icons'

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'
type ButtonSize = 'sm' | 'md' | 'lg'

const buttonVariants: Record<ButtonVariant, string> = {
  primary: 'bg-primary text-white hover:bg-primary-hover active:bg-primary-active',
  secondary: 'border border-line bg-white text-primary hover:bg-brand-50',
  ghost: 'text-body hover:bg-brand-50',
  danger: 'bg-danger text-white hover:brightness-95'
}

const buttonSizes: Record<ButtonSize, string> = {
  sm: 'px-3.5 py-1.5 text-[13px]',
  md: 'px-4 py-2 text-sm',
  lg: 'px-5 py-2.5 text-[15px]'
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant
  size?: ButtonSize
  icon?: IconName
  block?: boolean
}

export function Button({
  variant = 'primary',
  size = 'md',
  icon,
  block,
  className = '',
  children,
  ...rest
}: ButtonProps) {
  return (
    <button
      type="button"
      {...rest}
      className={`inline-flex items-center justify-center gap-1.5 rounded-lg font-bold transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:cursor-not-allowed disabled:opacity-40 ${buttonVariants[variant]} ${buttonSizes[size]} ${block ? 'w-full' : ''} ${className}`}
    >
      {icon && <Icon name={icon} size={16} />}
      {children}
    </button>
  )
}

type BadgeTone = 'success' | 'info' | 'neutral' | 'outline'

const badgeTones: Record<BadgeTone, string> = {
  success: 'bg-success-50 text-success',
  info: 'bg-brand-50 text-brand-700',
  neutral: 'bg-surface text-muted',
  outline: 'border border-line-strong text-muted'
}

export function Badge({ tone, dot, children }: { tone: BadgeTone; dot?: boolean; children: ReactNode }) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-bold ${badgeTones[tone]}`}
    >
      {dot && <span className="size-1.5 rounded-full bg-current" />}
      {children}
    </span>
  )
}

export function Card({
  title,
  aside,
  children,
  className = ''
}: {
  title?: string
  aside?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <section className={`rounded-xl border border-line bg-white shadow-card ${className}`}>
      {title && (
        <header className="flex items-center justify-between gap-3 border-b border-line px-6 py-4">
          <h2 className="text-base font-bold text-strong">{title}</h2>
          {aside}
        </header>
      )}
      <div className="px-6 py-5">{children}</div>
    </section>
  )
}

type AlertTone = 'info' | 'warning' | 'danger'

const alertTones: Record<AlertTone, { box: string; icon: string; name: IconName }> = {
  info: { box: 'border-brand-300/60 bg-brand-50', icon: 'text-primary', name: 'info' },
  warning: { box: 'border-warning/50 bg-warning-50', icon: 'text-warning-ink', name: 'alert' },
  danger: { box: 'border-danger/40 bg-danger-50', icon: 'text-danger', name: 'alert' }
}

export function Alert({ tone, title, children }: { tone: AlertTone; title?: string; children: ReactNode }) {
  const style = alertTones[tone]
  return (
    <div
      role={tone === 'info' ? 'note' : 'alert'}
      className={`flex gap-3 rounded-lg border px-4 py-3 text-[13.5px] ${style.box}`}
    >
      <Icon name={style.name} size={18} className={`mt-0.5 shrink-0 ${style.icon}`} />
      <div className="leading-relaxed text-body">
        {title && <p className="font-bold text-strong">{title}</p>}
        <div>{children}</div>
      </div>
    </div>
  )
}

export function Code({ children }: { children: ReactNode }) {
  return <code className="rounded-md bg-surface px-2 py-0.5 font-mono text-[12.5px] text-brand-700">{children}</code>
}

export function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) {
      return
    }
    const timer = setTimeout(() => setCopied(false), 1600)
    return () => clearTimeout(timer)
  }, [copied])
  const copy = async () => {
    await navigator.clipboard.writeText(value)
    setCopied(true)
  }
  return (
    <Button variant="secondary" size="sm" onClick={copy} className="shrink-0">
      {copied ? 'コピーしました' : 'コピー'}
    </Button>
  )
}

export function ErrorText({ message }: { message: string | null }) {
  return message ? <p className="text-[13px] text-danger">{message}</p> : null
}

type ModalProps = {
  title: string
  subtitle?: ReactNode
  width: string
  onClose?: () => void
  footer: ReactNode
  children: ReactNode
}

export function Modal({ title, subtitle, width, onClose, footer, children }: ModalProps) {
  const dialog = useRef<HTMLDivElement>(null)
  useEffect(() => {
    dialog.current?.querySelector<HTMLElement>('input, button:not([disabled])')?.focus()
  }, [])
  useEffect(() => {
    if (!onClose) {
      return
    }
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onClose()
      }
    }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [onClose])
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-scrim p-6">
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={`flex w-full animate-pop flex-col gap-4.5 rounded-2xl bg-white p-7 shadow-modal ${width}`}
      >
        <div>
          <h2 className="text-xl font-bold text-strong">{title}</h2>
          {subtitle && <p className="mt-1 text-[13px] text-muted">{subtitle}</p>}
        </div>
        {children}
        <div className="flex justify-end gap-2">{footer}</div>
      </div>
    </div>
  )
}
