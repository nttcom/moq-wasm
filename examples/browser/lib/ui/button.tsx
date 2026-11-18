import * as React from 'react'
import { cn } from './classNames'

const BUTTON_VARIANT_CLASSES = {
  default: 'bg-accent text-on-accent shadow-sm hover:bg-accent-hover',
  outline: 'border border-line-strong bg-raised text-fg hover:bg-hover'
}

const BUTTON_SIZE_CLASSES = {
  default: 'h-9 px-4',
  sm: 'h-8 px-3 text-[13px]',
  lg: 'h-11 px-6 text-[15px]'
}

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: keyof typeof BUTTON_VARIANT_CLASSES
  size?: keyof typeof BUTTON_SIZE_CLASSES
}

export function Button({ className, variant = 'default', size = 'default', ...props }: ButtonProps) {
  return (
    <button
      className={cn(
        'inline-flex cursor-pointer items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-semibold transition-colors disabled:pointer-events-none disabled:opacity-50',
        BUTTON_VARIANT_CLASSES[variant],
        BUTTON_SIZE_CLASSES[size],
        className
      )}
      {...props}
    />
  )
}
