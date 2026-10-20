// 原型（一次性，勿并入正式代码）
//
// 三个结构不同的「固定上游 IP」模态框变体，挂在真实的账号页上，用 ?variant=A|B|C 切换。
// 问题：用户在弹窗里要同时填 IP 组、超时、状态码、开关 —— 这四件事在同一个弹窗里的
// 信息层级应该怎么排？哪种排法让人一眼看出「哪些是必填、哪些有默认值、当前生效的是什么」？
//
// 每个变体都必须走真实的 /api/pinned-upstream-ip 读写（后端链路已上线并验证），
// 这样你能看到真实保存结果，而不是一个空壳表单。
//
// 为什么是一个可粘贴的模块：ui-islands 的构建产物要入库且 CI 逐字节比对，
// 把变体直接写进 accounts-page 会污染真实产物。这里做成独立组件，
// 由原型入口按 ?variant= 挂载。

import * as React from 'react'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@ui/dialog'
import { Button } from '@ui/button'
import { Switch } from '@ui/switch'
import { Input } from '@ui/input'
import { Textarea } from '@ui/textarea'
import { cn } from '@ui/lib/cn'

/**
 * 后端 GET/PUT 的响应形状（见 api/pinned_upstream.rs 的 pinned_json）。
 *
 * 注意：网页端桥接（web_shim 的 httpCall）与桌面壳的 `api_request` 都**会剥掉
 * `{success, data}` 信封**，交到这里的就是 `data` 本身。所以这里的类型只写
 * 内层形状 —— 按带信封的形状写会让 `payload.pinnedUpstreamIp` 是 undefined，
 * 渲染直接白屏（原型第一版就是这么翻车的）。
 */
export type PinnedSettings = {
  enabled: boolean
  ips: string[]
  waitTimeoutSeconds: number
  statusCodes: number[]
  currentIp: string
}

export type PinnedPayload = {
  pinnedUpstreamIp: PinnedSettings
  globalWaitTimeoutSeconds: number
  effectiveWaitTimeoutSeconds: number
}

export type VariantProps = {
  /** 当前生效的设置（由宿主页面读出后传进来） */
  settings: PinnedSettings
  /** 全局「等待响应超时」秒数，用于渲染「0 = 跟随全局（当前 N 秒）」 */
  globalWaitTimeoutSeconds: number
  onClose: () => void
  onSaved: (payload: PinnedPayload) => void
}

export type BridgeWindow = Window & {
  __TAURI_INTERNALS__?: { invoke?: (command: string, args: unknown) => Promise<unknown> }
}

/** 调后端。网页端桥与桌面壳都会剥掉 `{success,data}` 信封（见 PinnedPayload 的说明）。 */
async function invokeApi(method: string, body: unknown): Promise<unknown> {
  const bridge = (window as BridgeWindow).__TAURI_INTERNALS__
  if (!bridge?.invoke) throw new Error('这个页面没有注入桥接（window.__TAURI_INTERNALS__ 不存在）')
  return bridge.invoke('api_request', {
    request: { method, path: '/api/pinned-upstream-ip', body },
  })
}

const IPV4 = /^(\d{1,3}\.){3}\d{1,3}$/

function isIpv4(text: string): boolean {
  if (!IPV4.test(text)) return false
  return text.split('.').every(part => {
    const value = Number(part)
    return Number.isInteger(value) && value >= 0 && value <= 255
  })
}

/** 把多行文本按行拆成 IP 列表（去空白、去空行、去重，保序）。 */
export function parseIps(text: string): { ips: string[]; invalid: string[] } {
  const ips: string[] = []
  const invalid: string[] = []
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    if (!isIpv4(line)) {
      invalid.push(line)
      continue
    }
    if (!ips.includes(line)) ips.push(line)
  }
  return { ips, invalid }
}

export function parseCodes(text: string): { codes: number[]; invalid: string[] } {
  const codes: number[] = []
  const invalid: string[] = []
  for (const raw of text.split(',')) {
    const line = raw.trim()
    if (!line) continue
    const value = Number(line)
    if (!Number.isInteger(value) || value < 100 || value > 599) {
      invalid.push(line)
      continue
    }
    if (!codes.includes(value)) codes.push(value)
  }
  codes.sort((a, b) => a - b)
  return { codes, invalid }
}

// ─────────────────────────────────────────────────────────────────────
// 变体 A：分组卡片式 —— 三块独立设置区，每块自带标题与说明
//
// 结构特征：纵向三段（地址 / 触发条件 / 开关），每段一张内嵌卡片；
// 开关放在最后，读起来是「配好了再打开」。适合「一次配好、很少再改」。
// ─────────────────────────────────────────────────────────────────────

export function VariantA({ settings, globalWaitTimeoutSeconds, onClose, onSaved }: VariantProps) {
  const [enabled, setEnabled] = React.useState(settings.enabled)
  const [ipText, setIpText] = React.useState(settings.ips.join('\n'))
  const [timeout, setTimeoutText] = React.useState(String(settings.waitTimeoutSeconds))
  const [codeText, setCodeText] = React.useState(settings.statusCodes.join(', '))
  const [saving, setSaving] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)

  const parsedIps = parseIps(ipText)
  const parsedCodes = parseCodes(codeText)

  async function save() {
    if (saving) return
    if (enabled && parsedIps.ips.length === 0) {
      setStatus(<span className='text-destructive'>❌ 已开启，请至少填写一个 IP 地址</span>)
      return
    }
    if (parsedIps.invalid.length) {
      setStatus(<span className='text-destructive'>❌ 不是合法 IPv4：{parsedIps.invalid.join('、')}</span>)
      return
    }
    if (parsedCodes.invalid.length) {
      setStatus(<span className='text-destructive'>❌ 状态码不合法：{parsedCodes.invalid.join('、')}</span>)
      return
    }
    setSaving(true)
    setStatus(null)
    try {
      const payload = {
        enabled,
        ips: parsedIps.ips,
        waitTimeoutSeconds: Number(timeout) || 0,
        statusCodes: parsedCodes.codes,
      }
      const response = await invokeApi('PUT', payload)
      onSaved(response as PinnedPayload)
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {String(error)}</span>)
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>固定上游 IP</DialogTitle>
      </DialogHeader>
      <DialogBody className='flex flex-col gap-4'>
        <Section title='上游地址' hint='每行一个 IPv4，顺序就是轮换顺序'>
          <Textarea
            value={ipText}
            onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setIpText(event.target.value)}
            rows={5}
            className='font-mono text-xs'
            placeholder={'1.2.3.4\n5.6.7.8'}
          />
          <div className='mt-1 text-xs text-text-3'>
            已识别 {parsedIps.ips.length} 个
            {parsedIps.invalid.length > 0 && (
              <span className='text-destructive'> · {parsedIps.invalid.length} 行不合法</span>
            )}
            {settings.currentIp && ` · 当前 ${settings.currentIp}`}
          </div>
        </Section>

        <Section title='什么时候换一条线路' hint='任一命中就切 IP 并换账号'>
          <div className='grid grid-cols-2 gap-3'>
            <label className='flex flex-col gap-1 text-xs'>
              <span className='text-text-2'>等待响应超过（秒）</span>
              <Input
                value={timeout}
                onChange={(event: React.ChangeEvent<HTMLInputElement>) => setTimeoutText(event.target.value)}
                inputMode='numeric'
              />
              <span className='text-text-3'>0 = 跟随全局（当前 {globalWaitTimeoutSeconds} 秒）</span>
            </label>
            <label className='flex flex-col gap-1 text-xs'>
              <span className='text-text-2'>切换状态码</span>
              <Input
                value={codeText}
                onChange={(event: React.ChangeEvent<HTMLInputElement>) => setCodeText(event.target.value)}
                placeholder='502, 504'
                className='font-mono'
              />
              <span className='text-text-3'>英文逗号分隔</span>
            </label>
          </div>
        </Section>

        <Section title='开关' hint='关掉时保留配置，随时可再打开'>
          <div className='flex items-center gap-2'>
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label='启用固定上游 IP' />
            <span className='text-sm'>{enabled ? '已开启' : '已关闭'}</span>
          </div>
        </Section>
      </DialogBody>
      <DialogFooter>
        {status}
        <div className='mr-auto' />
        <Button variant='ghost' onClick={onClose} disabled={saving}>取消</Button>
        <Button onClick={save} disabled={saving}>{saving ? '保存中…' : '保存'}</Button>
      </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Section({ title, hint, children }: { title: string; hint: string; children: React.ReactNode }) {
  return (
    <div className='rounded-lg border border-border bg-surface-2 p-3'>
      <div className='mb-2 flex items-baseline gap-2'>
        <span className='text-sm font-medium'>{title}</span>
        <span className='text-xs text-text-3'>{hint}</span>
      </div>
      {children}
    </div>
  )
}

// ─────────────────────────────────────────────────────────────────────
// 变体 B：左右分栏 —— 左侧填地址（占大头），右侧是一列「规则」紧凑控件
//
// 结构特征：横向两栏，地址列表是主角（占 60% 宽且更高），规则挤在右侧窄栏里。
// 开关挪到标题行右侧，一眼可见「开没开」。适合「地址是重点、规则是附带」。
// ─────────────────────────────────────────────────────────────────────

export function VariantB({ settings, globalWaitTimeoutSeconds, onClose, onSaved }: VariantProps) {
  const [enabled, setEnabled] = React.useState(settings.enabled)
  const [ipText, setIpText] = React.useState(settings.ips.join('\n'))
  const [timeout, setTimeoutText] = React.useState(String(settings.waitTimeoutSeconds))
  const [codeText, setCodeText] = React.useState(settings.statusCodes.join(', '))
  const [saving, setSaving] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)

  const parsedIps = parseIps(ipText)
  const parsedCodes = parseCodes(codeText)

  async function save() {
    if (saving) return
    if (enabled && parsedIps.ips.length === 0) {
      setStatus(<span className='text-destructive'>❌ 已开启，请至少填写一个 IP 地址</span>)
      return
    }
    if (parsedIps.invalid.length) {
      setStatus(<span className='text-destructive'>❌ 不是合法 IPv4：{parsedIps.invalid.join('、')}</span>)
      return
    }
    setSaving(true)
    setStatus(null)
    try {
      const payload = {
        enabled,
        ips: parsedIps.ips,
        waitTimeoutSeconds: Number(timeout) || 0,
        statusCodes: parsedCodes.codes,
      }
      const response = await invokeApi('PUT', payload)
      onSaved(response as PinnedPayload)
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {String(error)}</span>)
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent showCloseButton={false}>
      <DialogHeader>
        <div className='flex w-full items-center gap-3'>
          <DialogTitle>固定上游 IP</DialogTitle>
          <div className='ml-auto flex items-center gap-2'>
            <span className='text-xs text-text-3'>{enabled ? '已开启' : '已关闭'}</span>
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label='启用固定上游 IP' />
          </div>
        </div>
      </DialogHeader>
      <DialogBody>
        <div className='flex gap-4'>
          <div className='flex min-w-0 flex-[3] flex-col gap-1'>
            <div className='flex items-baseline justify-between'>
              <span className='text-sm font-medium'>上游地址</span>
              <span className='text-xs text-text-3'>每行一个</span>
            </div>
            <Textarea
              value={ipText}
              onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setIpText(event.target.value)}
              rows={9}
              className='font-mono text-xs'
              placeholder={'1.2.3.4\n5.6.7.8'}
            />
            <div className='text-xs text-text-3'>
              {parsedIps.ips.length} 个地址
              {parsedIps.invalid.length > 0 && (
                <span className='text-destructive'> · {parsedIps.invalid.length} 行不合法</span>
              )}
            </div>
          </div>

          <div className='flex flex-[2] flex-col gap-4'>
            <div className='flex flex-col gap-1'>
              <span className='text-sm font-medium'>等待响应超时</span>
              <div className='flex items-center gap-1'>
                <Input
                  value={timeout}
                  onChange={(event: React.ChangeEvent<HTMLInputElement>) => setTimeoutText(event.target.value)}
                  inputMode='numeric'
                  className='w-20'
                />
                <span className='text-xs text-text-3'>秒</span>
              </div>
              <span className='text-xs text-text-3'>0 = 跟随全局（{globalWaitTimeoutSeconds} 秒）</span>
            </div>

            <div className='flex flex-col gap-1'>
              <span className='text-sm font-medium'>切换状态码</span>
              <Input
                value={codeText}
                onChange={(event: React.ChangeEvent<HTMLInputElement>) => setCodeText(event.target.value)}
                placeholder='502, 504'
                className='font-mono'
              />
              <span className='text-xs text-text-3'>英文逗号分隔</span>
            </div>

            <div className='mt-auto rounded-md bg-surface-inset p-2 text-xs text-text-2'>
              任一条件命中：换下一个 IP <span className='text-text-3'>并</span> 换下一个账号
            </div>
          </div>
        </div>
      </DialogBody>
      <DialogFooter>
        {status}
        <div className='mr-auto' />
        <Button variant='ghost' onClick={onClose} disabled={saving}>取消</Button>
        <Button onClick={save} disabled={saving}>{saving ? '保存中…' : '保存'}</Button>
      </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// ─────────────────────────────────────────────────────────────────────
// 变体 C：表格式清单 —— 把 IP 当成可逐行编辑的条目，右侧实时预览切换顺序
//
// 结构特征：地址不是自由文本域，而是**逐行条目**（带序号、删除按钮），
// 旁边一条竖线展示「轮换顺序」的可视化。规则折叠成一个内联条。
// 适合「地址需要经常增删改、且想看清轮换顺序」。
// ─────────────────────────────────────────────────────────────────────

export function VariantC({ settings, globalWaitTimeoutSeconds, onClose, onSaved }: VariantProps) {
  const [enabled, setEnabled] = React.useState(settings.enabled)
  const [rows, setRows] = React.useState<string[]>(
    settings.ips.length ? settings.ips : [''],
  )
  const [timeout, setTimeoutText] = React.useState(String(settings.waitTimeoutSeconds))
  const [codeText, setCodeText] = React.useState(settings.statusCodes.join(', '))
  const [saving, setSaving] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)

  const parsedCodes = parseCodes(codeText)
  const validIps = rows.map(row => row.trim()).filter(row => row && isIpv4(row))
  const invalidRows = rows.map(row => row.trim()).filter(row => row && !isIpv4(row))

  function updateRow(index: number, value: string) {
    setRows(previous => previous.map((row, position) => (position === index ? value : row)))
  }

  function addRow() {
    setRows(previous => [...previous, ''])
  }

  function removeRow(index: number) {
    setRows(previous => (previous.length <= 1 ? [''] : previous.filter((_, position) => position !== index)))
  }

  async function save() {
    if (saving) return
    if (enabled && validIps.length === 0) {
      setStatus(<span className='text-destructive'>❌ 已开启，请至少填写一个有效的 IP</span>)
      return
    }
    if (invalidRows.length) {
      setStatus(<span className='text-destructive'>❌ 不是合法 IPv4：{invalidRows.join('、')}</span>)
      return
    }
    setSaving(true)
    setStatus(null)
    try {
      const payload = {
        enabled,
        ips: validIps,
        waitTimeoutSeconds: Number(timeout) || 0,
        statusCodes: parsedCodes.codes,
      }
      const response = await invokeApi('PUT', payload)
      onSaved(response as PinnedPayload)
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {String(error)}</span>)
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent showCloseButton={false}>
      <DialogHeader>
        <DialogTitle>固定上游 IP</DialogTitle>
      </DialogHeader>
      <DialogBody className='flex flex-col gap-3'>
        <div className='flex gap-4'>
          <div className='flex min-w-0 flex-[3] flex-col gap-2'>
            <div className='flex items-baseline justify-between'>
              <span className='text-sm font-medium'>地址清单</span>
              <span className='text-xs text-text-3'>顺序即轮换顺序</span>
            </div>
            <div className='flex flex-col gap-1.5'>
              {rows.map((row, index) => {
                const trimmed = row.trim()
                const bad = Boolean(trimmed) && !isIpv4(trimmed)
                const isCurrent = Boolean(trimmed) && trimmed === settings.currentIp
                return (
                  <div key={index} className='flex items-center gap-2'>
                    <span className='w-5 shrink-0 text-right font-mono text-xs text-text-3'>{index + 1}</span>
                    <Input
                      value={row}
                      onChange={(event: React.ChangeEvent<HTMLInputElement>) => updateRow(index, event.target.value)}
                      placeholder='1.2.3.4'
                      className={cn('font-mono text-xs', bad && 'border-destructive-bd')}
                    />
                    {isCurrent && (
                      <span className='shrink-0 rounded-sm bg-ok-soft px-1.5 py-0.5 text-[10px] text-ok'>当前</span>
                    )}
                    <button
                      type='button'
                      onClick={() => removeRow(index)}
                      className='shrink-0 rounded-sm px-1.5 text-xs text-text-3 hover:text-destructive'
                      title='删除这一行'
                    >
                      ✕
                    </button>
                  </div>
                )
              })}
            </div>
            <Button variant='ghost' size='sm' onClick={addRow} className='self-start'>+ 添加一行</Button>
          </div>

          <div className='flex flex-[2] flex-col gap-2 border-l border-hairline pl-4'>
            <span className='text-sm font-medium'>轮换预览</span>
            <div className='flex flex-col gap-1 text-xs'>
              {validIps.length === 0 && <span className='text-text-3'>还没有有效地址</span>}
              {validIps.length > 0 && settings.currentIp && (
                <span className='text-text-2'>
                  当前 → <span className='font-mono'>{settings.currentIp}</span>
                </span>
              )}
              {validIps.map((ip, index) => (
                <div key={ip} className='flex items-center gap-1.5'>
                  <span className='text-text-3'>{index === 0 ? '先' : `再 ${index} 次后`}</span>
                  <span className='font-mono text-text-2'>{ip}</span>
                </div>
              ))}
              <span className='mt-1 text-text-3'>轮完一圈从头继续</span>
            </div>
          </div>
        </div>

        <div className='flex items-end gap-3 rounded-md bg-surface-inset p-2'>
          <label className='flex flex-col gap-0.5'>
            <span className='text-xs text-text-2'>等待响应超时（秒）</span>
            <Input
              value={timeout}
              onChange={(event: React.ChangeEvent<HTMLInputElement>) => setTimeoutText(event.target.value)}
              inputMode='numeric'
              className='w-20'
            />
          </label>
          <span className='pb-1.5 text-xs text-text-3'>0 = 跟随全局（{globalWaitTimeoutSeconds} 秒）</span>
          <label className='ml-auto flex flex-col gap-0.5'>
            <span className='text-xs text-text-2'>切换状态码</span>
            <Input
              value={codeText}
              onChange={(event: React.ChangeEvent<HTMLInputElement>) => setCodeText(event.target.value)}
              className='w-32 font-mono'
            />
          </label>
          <div className='flex items-center gap-2 pb-1.5'>
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label='启用固定上游 IP' />
            <span className='text-xs'>{enabled ? '已开启' : '已关闭'}</span>
          </div>
        </div>
      </DialogBody>
      <DialogFooter>
        {status}
        <div className='mr-auto' />
        <Button variant='ghost' onClick={onClose} disabled={saving}>取消</Button>
        <Button onClick={save} disabled={saving}>{saving ? '保存中…' : '保存'}</Button>
      </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/** 变体注册表：原型入口与切换条共用一份。 */
export const VARIANTS = {
  A: { component: VariantA, label: '分组卡片式' },
  B: { component: VariantB, label: '左右分栏' },
  C: { component: VariantC, label: '清单＋轮换预览' },
} as const

export type VariantKey = keyof typeof VARIANTS
