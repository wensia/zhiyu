import * as AlertDialog from "@radix-ui/react-alert-dialog"
import * as Dialog from "@radix-ui/react-dialog"
import * as DropdownPrimitive from "@radix-ui/react-dropdown-menu"
import * as SelectPrimitive from "@radix-ui/react-select"
import * as TabsPrimitive from "@radix-ui/react-tabs"
import * as ToastPrimitive from "@radix-ui/react-toast"
import { CalendarIcon, CheckIcon, ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon, EllipsisIcon, EyeIcon, EyeOffIcon, XIcon } from "lucide-react"
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type ComponentProps,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
  type MouseEvent,
  type SyntheticEvent,
  type TextareaHTMLAttributes,
} from "react"

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "outline" | "ghost" | "destructive"
  size?: "default" | "sm" | "icon" | "icon-sm"
}

export function Button({ variant = "outline", size = "default", className = "", type, ...props }: ButtonProps) {
  return <button className={`button button-${variant} button-size-${size} ${className}`} type={type || "button"} {...props} />
}

export function Field({
  label,
  error,
  description,
  className = "",
  children,
}: {
  label: string
  error?: string
  description?: string
  className?: string
  children: ReactNode
}) {
  return (
    <label className={`field ${className}`}>
      <span className="field-label">{label}</span>
      {children}
      {description ? <span className="field-description">{description}</span> : null}
      {error ? <span className="field-error">{error}</span> : null}
    </label>
  )
}

export function Input({ className = "", ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={`input ${className}`} {...props} />
}

function formatDate(date: Date) {
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${date.getFullYear()}-${month}-${day}`
}

function parseDate(value: string) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!match) return undefined
  const [, yearText, monthText, dayText] = match
  const year = Number(yearText)
  const month = Number(monthText)
  const day = Number(dayText)
  const date = new Date(year, month - 1, day)
  if (date.getFullYear() !== year || date.getMonth() !== month - 1 || date.getDate() !== day) return undefined
  return date
}

function startOfMonth(date: Date) {
  return new Date(date.getFullYear(), date.getMonth(), 1)
}

function shiftDateByMonths(date: Date, offset: number) {
  const targetMonth = new Date(date.getFullYear(), date.getMonth() + offset, 1)
  const lastDay = new Date(targetMonth.getFullYear(), targetMonth.getMonth() + 1, 0).getDate()
  return new Date(targetMonth.getFullYear(), targetMonth.getMonth(), Math.min(date.getDate(), lastDay))
}

function monthDays(month: Date) {
  const year = month.getFullYear()
  const monthIndex = month.getMonth()
  const firstWeekday = (new Date(year, monthIndex, 1).getDay() + 6) % 7
  const start = new Date(year, monthIndex, 1 - firstWeekday)
  return Array.from({ length: 42 }, (_, index) => new Date(start.getFullYear(), start.getMonth(), start.getDate() + index))
}

export function DatePicker({
  value,
  onValueChange,
  ariaLabel,
  placeholder = "选择日期",
  clearable = false,
  disabled = false,
  minDate,
  maxDate,
}: {
  value: string
  onValueChange: (value: string) => void
  ariaLabel: string
  placeholder?: string
  clearable?: boolean
  disabled?: boolean
  minDate?: string
  maxDate?: string
}) {
  const selected = parseDate(value)
  const minimum = minDate ? parseDate(minDate) : undefined
  const maximum = maxDate ? parseDate(maxDate) : undefined
  const [open, setOpen] = useState(false)
  const [month, setMonth] = useState(() => startOfMonth(selected || new Date()))
  const [view, setView] = useState<"days" | "months" | "years">("days")
  const panelRef = useRef<HTMLDivElement>(null)
  const shownValue = value || placeholder
  const days = monthDays(month)
  const decadeStart = Math.floor(month.getFullYear() / 10) * 10
  const years = Array.from({ length: 12 }, (_, index) => decadeStart - 1 + index)
  const dateOrder = (date: Date) => Number(formatDate(date).replaceAll("-", ""))
  const isDateEnabled = (date: Date) => (!minimum || dateOrder(date) >= dateOrder(minimum)) && (!maximum || dateOrder(date) <= dateOrder(maximum))
  const monthHasEnabledDate = (date: Date) => {
    const first = startOfMonth(date)
    const last = new Date(first.getFullYear(), first.getMonth() + 1, 0)
    return (!minimum || dateOrder(last) >= dateOrder(minimum)) && (!maximum || dateOrder(first) <= dateOrder(maximum))
  }
  const yearHasEnabledDate = (year: number) => {
    const first = new Date(year, 0, 1)
    const last = new Date(year, 11, 31)
    return (!minimum || dateOrder(last) >= dateOrder(minimum)) && (!maximum || dateOrder(first) <= dateOrder(maximum))
  }
  const decadeHasEnabledDate = (start: number) => Array.from({ length: 12 }, (_, index) => start - 1 + index).some(yearHasEnabledDate)
  const queuePanelFocus = (selector: string) => window.requestAnimationFrame(() => panelRef.current?.querySelector<HTMLButtonElement>(selector)?.focus())
  const focusableDay = days.find((date) => formatDate(date) === value && isDateEnabled(date))
    || days.find((date) => date.getMonth() === month.getMonth() && isDateEnabled(date))
    || days.find(isDateEnabled)
  const moveView = (offset: -1 | 1) => {
    if (view === "days") {
      const next = new Date(month.getFullYear(), month.getMonth() + offset, 1)
      if (!monthHasEnabledDate(next)) return
      setMonth(next)
      queuePanelFocus('.date-picker-day[tabindex="0"]')
      return
    }
    if (view === "months") {
      const next = new Date(month.getFullYear() + offset, month.getMonth(), 1)
      if (!yearHasEnabledDate(next.getFullYear())) return
      setMonth(next)
      queuePanelFocus(`[data-month="${next.getMonth()}"]`)
      return
    }
    const next = new Date(month.getFullYear() + offset * 10, month.getMonth(), 1)
    if (!decadeHasEnabledDate(Math.floor(next.getFullYear() / 10) * 10)) return
    setMonth(next)
    queuePanelFocus(`[data-year="${next.getFullYear()}"]`)
  }
  const previousDisabled = view === "days"
    ? !monthHasEnabledDate(new Date(month.getFullYear(), month.getMonth() - 1, 1))
    : view === "months"
      ? !yearHasEnabledDate(month.getFullYear() - 1)
      : !decadeHasEnabledDate(decadeStart - 10)
  const nextDisabled = view === "days"
    ? !monthHasEnabledDate(new Date(month.getFullYear(), month.getMonth() + 1, 1))
    : view === "months"
      ? !yearHasEnabledDate(month.getFullYear() + 1)
      : !decadeHasEnabledDate(decadeStart + 10)
  const previousLabel = view === "days" ? "上个月" : view === "months" ? "上一年" : "上一个十年"
  const nextLabel = view === "days" ? "下个月" : view === "months" ? "下一年" : "下一个十年"
  const selectDate = (date: Date) => {
    if (!isDateEnabled(date)) return
    onValueChange(formatDate(date))
    setOpen(false)
  }
  const handleOpenChange = (nextOpen: boolean) => {
    setOpen(nextOpen)
    if (!nextOpen) return
    setMonth(startOfMonth(selected || new Date()))
    setView("days")
  }
  const openMonthView = () => {
    setView("months")
    queuePanelFocus(`[data-month="${month.getMonth()}"]`)
  }
  const openYearView = () => {
    setView("years")
    queuePanelFocus(`[data-year="${month.getFullYear()}"]`)
  }
  const handleGridKeyDown = (event: KeyboardEvent<HTMLButtonElement>, columns: number) => {
    const movement = event.key === "ArrowLeft" ? -1
      : event.key === "ArrowRight" ? 1
        : event.key === "ArrowUp" ? -columns
          : event.key === "ArrowDown" ? columns
            : 0
    if (!movement) return
    event.preventDefault()
    const buttons = [...(event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button") || [])]
    let index = buttons.indexOf(event.currentTarget) + movement
    while (index >= 0 && index < buttons.length && buttons[index].disabled) index += movement > 0 ? 1 : -1
    buttons[index]?.focus()
  }
  const handleDayKeyDown = (event: KeyboardEvent<HTMLButtonElement>, date: Date) => {
    let target: Date | undefined
    if (event.key === "ArrowLeft") target = new Date(date.getFullYear(), date.getMonth(), date.getDate() - 1)
    if (event.key === "ArrowRight") target = new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1)
    if (event.key === "ArrowUp") target = new Date(date.getFullYear(), date.getMonth(), date.getDate() - 7)
    if (event.key === "ArrowDown") target = new Date(date.getFullYear(), date.getMonth(), date.getDate() + 7)
    if (event.key === "Home") target = new Date(date.getFullYear(), date.getMonth(), date.getDate() - ((date.getDay() + 6) % 7))
    if (event.key === "End") target = new Date(date.getFullYear(), date.getMonth(), date.getDate() + (6 - ((date.getDay() + 6) % 7)))
    if (event.key === "PageUp") target = shiftDateByMonths(date, event.shiftKey ? -12 : -1)
    if (event.key === "PageDown") target = shiftDateByMonths(date, event.shiftKey ? 12 : 1)
    if (!target) return
    event.preventDefault()
    if (!isDateEnabled(target)) return
    setMonth(startOfMonth(target))
    queuePanelFocus(`[data-date="${formatDate(target)}"]`)
  }
  const clear = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault()
    event.stopPropagation()
    onValueChange("")
  }

  return (
    <Dialog.Root open={open} onOpenChange={handleOpenChange}>
      <div className="date-picker-control">
        <Dialog.Trigger asChild>
          <button aria-label={ariaLabel} className={`date-picker-trigger ${clearable && value ? "date-picker-trigger-clearable" : ""} ${value ? "" : "date-picker-placeholder"}`} disabled={disabled} title={value || undefined} type="button">
            <CalendarIcon aria-hidden="true" />
            <span>{shownValue}</span>
          </button>
        </Dialog.Trigger>
        {clearable && value ? <button aria-label={`清空${ariaLabel}`} className="date-picker-clear" disabled={disabled} onClick={clear} type="button"><XIcon /></button> : null}
      </div>
      <Dialog.Portal>
        <Dialog.Overlay className="overlay" />
        <Dialog.Content
          aria-describedby={undefined}
          className="date-picker-dialog"
          onEscapeKeyDown={(event) => {
            if (view === "days") return
            event.preventDefault()
            setView(view === "years" ? "months" : "days")
            queuePanelFocus(view === "years" ? `[data-month="${month.getMonth()}"]` : '.date-picker-day[tabindex="0"]')
          }}
          onOpenAutoFocus={(event) => {
            event.preventDefault()
            queuePanelFocus('.date-picker-day[tabindex="0"]')
          }}
        >
          <Dialog.Title>选择日期</Dialog.Title>
          <div className="date-picker-month-nav">
            <Button aria-label={previousLabel} disabled={previousDisabled} onClick={() => moveView(-1)} size="icon-sm"><ChevronLeftIcon /></Button>
            {view === "days" ? (
              <button aria-label={`选择年月，当前 ${month.getFullYear()} 年 ${month.getMonth() + 1} 月`} className="date-picker-period-trigger" onClick={openMonthView} type="button">
                <span>{month.getFullYear()} 年 {month.getMonth() + 1} 月</span><ChevronDownIcon aria-hidden="true" />
              </button>
            ) : view === "months" ? (
              <button aria-label={`选择年份，当前 ${month.getFullYear()} 年`} className="date-picker-period-trigger" onClick={openYearView} type="button">
                <span>{month.getFullYear()} 年</span><ChevronDownIcon aria-hidden="true" />
              </button>
            ) : <span className="date-picker-period-label">{decadeStart}–{decadeStart + 9} 年</span>}
            <Button aria-label={nextLabel} disabled={nextDisabled} onClick={() => moveView(1)} size="icon-sm"><ChevronRightIcon /></Button>
          </div>
          <div className="date-picker-panel" ref={panelRef}>
            {view === "days" ? (
              <div aria-label={`${month.getFullYear()} 年 ${month.getMonth() + 1} 月日历`} className="date-picker-calendar" role="grid">
                {["一", "二", "三", "四", "五", "六", "日"].map((day) => <span className="date-picker-weekday" key={day}>{day}</span>)}
                {days.map((date) => {
                  const dateValue = formatDate(date)
                  const isCurrentMonth = date.getMonth() === month.getMonth()
                  const isSelected = dateValue === value
                  return <button aria-current={dateValue === formatDate(new Date()) ? "date" : undefined} aria-label={dateValue} aria-pressed={isSelected} className={`date-picker-day ${isCurrentMonth ? "" : "date-picker-day-outside"}`} data-date={dateValue} disabled={!isDateEnabled(date)} key={dateValue} onClick={() => selectDate(date)} onKeyDown={(event) => handleDayKeyDown(event, date)} role="gridcell" tabIndex={focusableDay && formatDate(focusableDay) === dateValue ? 0 : -1} type="button">{date.getDate()}</button>
                })}
              </div>
            ) : view === "months" ? (
              <div aria-label={`${month.getFullYear()} 年月份`} className="date-picker-choice-grid" role="grid">
                {Array.from({ length: 12 }, (_, monthIndex) => {
                  const candidate = new Date(month.getFullYear(), monthIndex, 1)
                  const enabled = monthHasEnabledDate(candidate)
                  return <button aria-label={`${monthIndex + 1} 月`} aria-selected={monthIndex === month.getMonth()} className="date-picker-choice" data-month={monthIndex} disabled={!enabled} key={monthIndex} onClick={() => { setMonth(candidate); setView("days"); queuePanelFocus('.date-picker-day[tabindex="0"]') }} onKeyDown={(event) => { if (event.key === "PageUp" || event.key === "PageDown") { event.preventDefault(); moveView(event.key === "PageUp" ? -1 : 1); return } handleGridKeyDown(event, 3) }} role="gridcell" tabIndex={monthIndex === month.getMonth() && enabled ? 0 : -1} type="button">{monthIndex + 1} 月</button>
                })}
              </div>
            ) : (
              <div aria-label={`选择年份，${decadeStart} 至 ${decadeStart + 9} 年`} className="date-picker-choice-grid" role="grid">
                {years.map((year) => {
                  const enabled = yearHasEnabledDate(year)
                  const outside = year < decadeStart || year > decadeStart + 9
                  return <button aria-label={`${year} 年`} aria-selected={year === month.getFullYear()} className={`date-picker-choice ${outside ? "date-picker-choice-outside" : ""}`} data-year={year} disabled={!enabled} key={year} onClick={() => { setMonth(new Date(year, month.getMonth(), 1)); setView("months"); queuePanelFocus(`[data-month="${month.getMonth()}"]`) }} onKeyDown={(event) => { if (event.key === "PageUp" || event.key === "PageDown") { event.preventDefault(); moveView(event.key === "PageUp" ? -1 : 1); return } handleGridKeyDown(event, 3) }} role="gridcell" tabIndex={year === month.getFullYear() && enabled ? 0 : -1} type="button">{year} 年</button>
                })}
              </div>
            )}
          </div>
          <span aria-live="polite" className="sr-only">{view === "days" ? `已显示 ${month.getFullYear()} 年 ${month.getMonth() + 1} 月` : view === "months" ? `正在选择 ${month.getFullYear()} 年的月份` : `正在选择 ${decadeStart} 至 ${decadeStart + 9} 年`}</span>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

export function PasswordInput(props: Omit<InputHTMLAttributes<HTMLInputElement>, "type">) {
  const [visible, setVisible] = useState(false)
  return (
    <div className="password-input">
      <Input {...props} type={visible ? "text" : "password"} />
      <Button
        aria-label={visible ? "隐藏密码" : "显示密码"}
        onClick={() => setVisible((value) => !value)}
        size="icon-sm"
        variant="ghost"
      >
        {visible ? <EyeOffIcon /> : <EyeIcon />}
      </Button>
    </div>
  )
}

export function Textarea({ className = "", ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea className={`textarea ${className}`} {...props} />
}

export function CheckboxControl({ label, className = "", ...props }: InputHTMLAttributes<HTMLInputElement> & { label: string }) {
  return <label className={`toggle ${className}`}><input type="checkbox" {...props} /><span>{label}</span></label>
}

export function TabsRoot(props: ComponentProps<typeof TabsPrimitive.Root>) {
  return <TabsPrimitive.Root {...props} />
}

export function TabsList(props: ComponentProps<typeof TabsPrimitive.List>) {
  return <TabsPrimitive.List {...props} />
}

export function TabsTrigger(props: ComponentProps<typeof TabsPrimitive.Trigger>) {
  return <TabsPrimitive.Trigger {...props} />
}

export function TabsContent(props: ComponentProps<typeof TabsPrimitive.Content>) {
  return <TabsPrimitive.Content {...props} />
}

export function ActionMenu({
  label,
  items,
}: {
  label: string
  items: Array<{ label: string; icon?: ReactNode; onSelect: () => void; destructive?: boolean }>
}) {
  return (
    <DropdownPrimitive.Root>
      <DropdownPrimitive.Trigger asChild><Button aria-label={label} size="icon-sm" variant="ghost"><EllipsisIcon /></Button></DropdownPrimitive.Trigger>
      <DropdownPrimitive.Portal>
        <DropdownPrimitive.Content align="end" className="menu" sideOffset={4}>
          {items.map((item) => <DropdownPrimitive.Item className={item.destructive ? "menu-item-destructive" : undefined} key={item.label} onSelect={item.onSelect}>{item.icon}{item.label}</DropdownPrimitive.Item>)}
        </DropdownPrimitive.Content>
      </DropdownPrimitive.Portal>
    </DropdownPrimitive.Root>
  )
}

export type SelectOption = { value: string; label: string }

type SelectProps = {
  value: string
  onValueChange: (value: string) => void
  options: SelectOption[]
  ariaLabel: string
  placeholder?: string
  className?: string
  disabled?: boolean
  size?: "default" | "sm"
  side?: "top" | "bottom"
  clearable?: boolean
  clearLabel?: string
}

export function Select({
  value,
  onValueChange,
  options,
  ariaLabel,
  placeholder,
  className = "",
  disabled = false,
  size = "default",
  side,
  clearable = false,
  clearLabel = "清空选择",
}: SelectProps) {
  const clear = (event: SyntheticEvent) => {
    event.preventDefault()
    event.stopPropagation()
    onValueChange("")
  }

  return (
    <SelectPrimitive.Root disabled={disabled} onValueChange={onValueChange} value={value}>
      <SelectPrimitive.Trigger
        aria-label={ariaLabel}
        className={`select-trigger select-trigger-${size} ${className}`}
      >
        <SelectPrimitive.Value placeholder={placeholder} />
        {clearable && value ? (
          <span
            aria-label={clearLabel}
            className="select-clear"
            onClick={clear}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") clear(event)
            }}
            onPointerDown={clear}
            role="button"
            tabIndex={-1}
          >
            <XIcon />
          </span>
        ) : (
          <SelectPrimitive.Icon className="select-icon"><ChevronDownIcon /></SelectPrimitive.Icon>
        )}
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          align="start"
          className="select-content"
          position="popper"
          side={side}
          sideOffset={4}
        >
          <SelectPrimitive.Viewport className="select-viewport">
            {options.map((option) => (
              <SelectPrimitive.Item className="select-item" key={option.value} value={option.value}>
                <SelectPrimitive.ItemText>{option.label}</SelectPrimitive.ItemText>
                <SelectPrimitive.ItemIndicator className="select-indicator"><CheckIcon /></SelectPrimitive.ItemIndicator>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  )
}

export function FilterSelect(props: Omit<SelectProps, "clearable">) {
  return <Select {...props} clearLabel={`清空${props.ariaLabel}筛选`} clearable />
}

export function Modal({
  open,
  onOpenChange,
  title,
  description,
  children,
  footer,
  wide = false,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description?: string
  children: ReactNode
  footer?: ReactNode
  wide?: boolean
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="overlay" />
        <Dialog.Content className={`dialog ${wide ? "dialog-wide" : ""}`}>
          <header className="dialog-header">
            <div>
              <Dialog.Title>{title}</Dialog.Title>
              {description ? <Dialog.Description>{description}</Dialog.Description> : null}
            </div>
            <Dialog.Close asChild><Button aria-label="关闭" size="icon-sm" variant="ghost"><XIcon /></Button></Dialog.Close>
          </header>
          <div className="dialog-body">{children}</div>
          {footer ? <footer className="dialog-footer">{footer}</footer> : null}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

export function Sheet({ open, onOpenChange, title, children }: { open: boolean; onOpenChange: (open: boolean) => void; title: string; children: ReactNode }) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="overlay" />
        <Dialog.Content className="sheet">
          <header className="dialog-header">
            <Dialog.Title>{title}</Dialog.Title>
            <Dialog.Close asChild><Button aria-label="关闭" size="icon-sm" variant="ghost"><XIcon /></Button></Dialog.Close>
          </header>
          <div className="sheet-body">{children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  onConfirm,
  pending = false,
  destructive = true,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description: string
  confirmLabel: string
  onConfirm: () => void
  pending?: boolean
  destructive?: boolean
}) {
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="overlay" />
        <AlertDialog.Content className="alert-dialog">
          <AlertDialog.Title>{title}</AlertDialog.Title>
          <AlertDialog.Description>{description}</AlertDialog.Description>
          <div className="dialog-footer borderless">
            <AlertDialog.Cancel asChild><Button disabled={pending}>取消</Button></AlertDialog.Cancel>
            <AlertDialog.Action asChild>
              <Button disabled={pending} variant={destructive ? "destructive" : "default"} onClick={onConfirm}>
                {pending ? "正在处理…" : confirmLabel}
              </Button>
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  )
}

export function InlineNotice({ type = "info", children }: { type?: "info" | "error" | "success"; children: ReactNode }) {
  return <div className={`notice notice-${type}`} role={type === "error" ? "alert" : "status"}>{children}</div>
}

export function TablePagination({
  page,
  pageSize,
  total,
  onPageChange,
  disabled = false,
}: {
  page: number
  pageSize: number
  total: number
  onPageChange: (page: number) => void
  disabled?: boolean
}) {
  const pageCount = Math.max(1, Math.ceil(total / pageSize))
  const current = Math.min(Math.max(page, 1), pageCount)
  const start = Math.max(1, Math.min(current - 1, pageCount - 2))
  const pages = Array.from({ length: Math.min(3, pageCount) }, (_, index) => start + index)
  const options = useMemo(() => Array.from({ length: pageCount }, (_, index) => ({ value: String(index + 1), label: `第 ${index + 1} 页` })), [pageCount])
  return (
    <footer className="pagination">
      <span>共 {total} 笔</span>
      {pageCount > 1 ? (
        <div className="pagination-pages">
          {pages.map((value) => (
            <Button
              aria-current={value === current ? "page" : undefined}
              aria-label={`第 ${value} 页`}
              disabled={disabled}
              key={value}
              onClick={() => onPageChange(value)}
              size="sm"
              variant={value === current ? "primary" : "outline"}
            >
              {value}
            </Button>
          ))}
          <Select
            ariaLabel="跳转页码"
            disabled={disabled}
            onValueChange={(value) => onPageChange(Number(value))}
            options={options}
            side="top"
            size="sm"
            value={String(current)}
          />
        </div>
      ) : <span aria-hidden="true" />}
    </footer>
  )
}

type ToastMessage = {
  title: string
  description?: string
  type?: "info" | "success" | "error"
}

type ToastItem = ToastMessage & { id: number }
const ToastContext = createContext<((message: ToastMessage) => void) | null>(null)

export function AppToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([])
  const notify = useCallback((message: ToastMessage) => {
    setItems((current) => [...current, { ...message, id: Date.now() + current.length }])
  }, [])
  return (
    <ToastContext.Provider value={notify}>
      <ToastPrimitive.Provider duration={3600} swipeDirection="right">
        {children}
        {items.map((item) => (
          <ToastPrimitive.Root
            className={`toast toast-${item.type || "info"}`}
            key={item.id}
            onOpenChange={(open) => {
              if (!open) setItems((current) => current.filter((entry) => entry.id !== item.id))
            }}
            open
          >
            <div>
              <ToastPrimitive.Title>{item.title}</ToastPrimitive.Title>
              {item.description ? <ToastPrimitive.Description>{item.description}</ToastPrimitive.Description> : null}
            </div>
            <ToastPrimitive.Close asChild><Button aria-label="关闭通知" size="icon-sm" variant="ghost"><XIcon /></Button></ToastPrimitive.Close>
          </ToastPrimitive.Root>
        ))}
        <ToastPrimitive.Viewport className="toast-viewport" />
      </ToastPrimitive.Provider>
    </ToastContext.Provider>
  )
}

// Toast state belongs to the shared UI layer; consumers only receive the stable notifier.
// eslint-disable-next-line react-refresh/only-export-components
export function useToast() {
  const notify = useContext(ToastContext)
  if (!notify) throw new Error("useToast must be used inside AppToastProvider")
  return notify
}
