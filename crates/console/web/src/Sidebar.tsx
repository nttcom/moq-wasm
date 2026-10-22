import { useEffect, useRef, useState } from 'react'

import type { ConsoleApp } from './api'
import { Icon } from './icons'
import { accountNav, appNav, type Page } from './navigation'

type Props = {
  apps: ConsoleApp[]
  current: ConsoleApp | null
  page: Page
  email: string | null
  onSelectApp: (appId: string) => void
  onNavigate: (page: Page) => void
  onCreateApp: () => void
}

export function Sidebar({ apps, current, page, email, onSelectApp, onNavigate, onCreateApp }: Props) {
  return (
    <aside className="flex h-screen flex-col bg-navy text-white">
      <div className="flex h-16 shrink-0 items-center gap-2.5 border-b border-white/12 px-5">
        <span className="grid size-7 place-items-center rounded-[7px] bg-primary text-xs font-bold">MQ</span>
        <span className="text-[15px] font-bold">MoQ Console</span>
      </div>
      <AppSwitcher
        apps={apps}
        current={current}
        onSelect={onSelectApp}
        onShowAll={() => onNavigate('apps')}
        onCreate={onCreateApp}
      />
      <nav aria-label="メニュー" className="flex flex-1 flex-col gap-0.5 overflow-y-auto px-2.5 py-2">
        {appNav.map((item) => (
          <NavButton key={item.page} {...item} active={page === item.page} onClick={() => onNavigate(item.page)} />
        ))}
        <p className="px-3 pt-3.5 pb-1.5 text-[10px] font-bold tracking-widest text-white/45">アカウント</p>
        {accountNav.map((item) => (
          <NavButton key={item.page} {...item} active={page === item.page} onClick={() => onNavigate(item.page)} />
        ))}
      </nav>
      <UserRow email={email} />
    </aside>
  )
}

type NavButtonProps = {
  label: string
  icon: Parameters<typeof Icon>[0]['name']
  soon?: boolean
  active: boolean
  onClick: () => void
}

function NavButton({ label, icon, soon, active, onClick }: NavButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? 'page' : undefined}
      className={`flex items-center gap-2.75 rounded-lg px-3 py-2.25 text-left text-[13.5px] font-medium transition-colors focus-visible:outline-2 focus-visible:outline-white ${
        active ? 'bg-primary text-white' : 'text-white/80 hover:bg-white/8'
      } ${soon && !active ? 'opacity-60' : ''}`}
    >
      <Icon name={icon} size={17} />
      <span className="flex-1">{label}</span>
      {soon && <span className="rounded-full bg-white/14 px-2 py-0.5 text-[10px] font-bold">準備中</span>}
    </button>
  )
}

type SwitcherProps = {
  apps: ConsoleApp[]
  current: ConsoleApp | null
  onSelect: (appId: string) => void
  onShowAll: () => void
  onCreate: () => void
}

function AppSwitcher({ apps, current, onSelect, onShowAll, onCreate }: SwitcherProps) {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) {
      return
    }
    const closeOutside = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node)) {
        setOpen(false)
      }
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', closeOutside)
    document.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('mousedown', closeOutside)
      document.removeEventListener('keydown', closeOnEscape)
    }
  }, [open])
  const choose = (action: () => void) => {
    setOpen(false)
    action()
  }
  return (
    <div ref={root} className="relative mx-3.5 mt-4 mb-1.5">
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className={`flex w-full items-center gap-2.5 rounded-lg border border-white/18 px-3 py-2.5 text-left focus-visible:outline-2 focus-visible:outline-white ${
          open ? 'bg-white/16' : 'bg-white/8 hover:bg-white/12'
        }`}
      >
        <span className="grid size-7 shrink-0 place-items-center rounded-md bg-white/10">
          <Icon name="box" size={16} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-bold">
            {current ? displayName(current) : 'app がありません'}
          </span>
          {current && <span className="block truncate font-mono text-[11px] text-white/60">{current.appId}</span>}
        </span>
        <Icon name="switch" size={16} className="shrink-0 text-white/70" />
      </button>
      {open && (
        <div
          role="menu"
          className="absolute inset-x-0 top-full z-20 mt-1.5 animate-pop rounded-[10px] border border-line bg-white py-2 text-body shadow-pop"
        >
          <p className="px-3.5 pt-1 pb-2 text-[10px] font-bold tracking-widest text-subtle">APP を切り替え</p>
          {apps.map((app) => (
            <button
              key={app.appId}
              type="button"
              role="menuitem"
              onClick={() => choose(() => onSelect(app.appId))}
              className="flex w-full items-start gap-2.5 px-3.5 py-2 text-left hover:bg-brand-50"
            >
              <span
                className={`mt-1.5 size-1.5 shrink-0 rounded-full ${app.appId === current?.appId ? 'bg-primary' : 'bg-line-strong'}`}
              />
              <span className="min-w-0">
                <span className="block truncate text-[13px] font-bold text-strong">{displayName(app)}</span>
                <span className="block truncate font-mono text-[11px] text-subtle">{app.appId}</span>
              </span>
            </button>
          ))}
          <div className="my-1.5 border-t border-divider" />
          <button
            type="button"
            role="menuitem"
            onClick={() => choose(onShowAll)}
            className="block w-full px-3.5 py-2 text-left text-[13px] hover:bg-brand-50"
          >
            すべての app
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => choose(onCreate)}
            className="flex w-full items-center gap-1.5 px-3.5 py-2 text-left text-[13px] font-bold text-primary hover:bg-brand-50"
          >
            <Icon name="plus" size={14} />
            app を作成
          </button>
        </div>
      )}
    </div>
  )
}

function UserRow({ email }: { email: string | null }) {
  const label = email ?? '開発用のアカウント'
  return (
    <div className="flex shrink-0 items-center gap-3 border-t border-white/12 px-4 py-3.5">
      <span className="grid size-8 shrink-0 place-items-center rounded-full bg-primary text-xs font-bold uppercase">
        {label.slice(0, 2)}
      </span>
      <span className="truncate text-[12px] text-white/80">{label}</span>
    </div>
  )
}

export function displayName(app: ConsoleApp): string {
  return app.name ?? app.appId
}
