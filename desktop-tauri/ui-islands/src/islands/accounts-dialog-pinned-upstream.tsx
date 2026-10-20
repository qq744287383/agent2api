/**
 * 固定上游 IP 的组级弹窗（账号页 WorkBuddy 国际版组的 ⋯ 菜单进入）。
 *
 * 布局是 B 变体（左右分栏）：左侧 IP 地址占大头，右侧一列紧凑的规则控件，
 * 开关放在标题行右侧。选它的理由是「地址是主角、规则是附带，开关状态要一眼可见」。
 * 三个候选变体的对比见一次性分支 prototype/pinned-upstream-modal。
 *
 * ── 保存时后端会做的事（界面上要让人有预期）──────────────
 *   · 开关打开但列表为空 → 后端 400，这里先在前端拦一次，省一次往返；
 *   · **IP 列表一变**，该组所有账号的独立 IP 会被重置为「跟随组」，组当前 IP
 *     也会从新列表里重选 —— 这是用户改完列表后最容易困惑的一点，保存成功后
 *     用 toast 明说。
 *
 * 术语见 GLOSSARY.md：固定上游 IP / 组策略 / 账号策略 / 切换状态码。
 */

import * as React from 'react'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Textarea,
  Switch,
} from '@ui'
import { cn } from '@ui/lib/cn'
import { shared, toast, errorMessage } from './accounts-shared'
import { t } from '../i18n'

type Props = {
  provider: string
  onClose: () => void
}

type Draft = {
  enabled: boolean
  ips: string[]
  waitTimeoutSeconds: number
  statusCodes: number[]
  currentIp: string
}

export function PinnedUpstreamDialog({ provider, onClose }: Props) {
  const [draft, setDraft] = React.useState<Draft | null>(null)
  const [ipText, setIpText] = React.useState('')
  const [timeoutText, setTimeoutText] = React.useState('0')
  const [codeText, setCodeText] = React.useState('502')
  const [globalSeconds, setGlobalSeconds] = React.useState(300)
  const [saving, setSaving] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)
  // 打开时的 IP 列表快照：保存时用它判断「列表是否变了」，变了才好提示重置
  const openedIps = React.useRef<string>('')

  React.useEffect(() => {
    let cancelled = false
    void (async () => {
      const response = await shared().workbuddyDesktop?.getPinnedUpstream?.()
      if (cancelled) return
      const settings = response?.pinnedUpstreamIp
      if (!response || !settings) {
        setStatus(<span className='text-destructive'>❌ {t('读取固定上游 IP 设置失败')}</span>)
        setDraft({ enabled: false, ips: [], waitTimeoutSeconds: 0, statusCodes: [502], currentIp: '' })
        return
      }
      openedIps.current = settings.ips.join(',')
      setGlobalSeconds(response.globalWaitTimeoutSeconds)
      setIpText(settings.ips.join('\n'))
      setTimeoutText(String(settings.waitTimeoutSeconds))
      setCodeText(settings.statusCodes.join(', '))
      setDraft(settings)
    })()
    return () => { cancelled = true }
  }, [])

  if (!draft) {
    return (
      <Dialog open onOpenChange={open => { if (!open) onClose() }}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>{t('固定上游 IP')}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <div className='text-sm text-text-3'>{t('读取中…')}</div>
          </DialogBody>
        </DialogContent>
      </Dialog>
    )
  }

  const parsedIps = parseIps(ipText)
  const parsedCodes = parseCodes(codeText)
  const enabled = draft.enabled

  async function save(): Promise<void> {
    if (saving) return
    if (enabled && parsedIps.ips.length === 0) {
      setStatus(<span className='text-destructive'>❌ {t('已开启，请至少填写一个 IP 地址')}</span>)
      return
    }
    if (parsedIps.invalid.length) {
      setStatus(
        <span className='text-destructive'>
          ❌ {t('不是合法 IPv4：{list}', { list: parsedIps.invalid.join('、') })}
        </span>,
      )
      return
    }
    if (parsedCodes.invalid.length) {
      setStatus(
        <span className='text-destructive'>
          ❌ {t('状态码不合法：{list}', { list: parsedCodes.invalid.join('、') })}
        </span>,
      )
      return
    }
    setSaving(true)
    setStatus(null)
    try {
      const saved = await shared().workbuddyDesktop?.savePinnedUpstream?.({
        enabled,
        ips: parsedIps.ips,
        waitTimeoutSeconds: Number(timeoutText) || 0,
        statusCodes: parsedCodes.codes,
      })
      const ipsChanged = parsedIps.ips.join(',') !== openedIps.current
      toast(
        ipsChanged
          ? t('已保存：IP 列表变了，组内账号的独立 IP 已重置为「跟随组」')
          : t('固定上游 IP 设置已保存'),
      )
      // 保存后刷新面板：组标题后的当前 IP 与账号列都依赖这份新值
      void saved
      await shared().wbApp?.refresh?.()
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <div className='flex w-full items-center gap-3'>
            <DialogTitle>{t('固定上游 IP')}</DialogTitle>
            <div className='ml-auto flex items-center gap-2'>
              <span className='text-xs text-text-3'>{enabled ? t('已开启') : t('已关闭')}</span>
              <Switch
                checked={enabled}
                onCheckedChange={next => setDraft({ ...draft, enabled: next })}
                aria-label={t('启用固定上游 IP')}
              />
            </div>
          </div>
        </DialogHeader>
        <DialogBody>
          <div className='flex gap-4'>
            {/* 左：IP 地址（主角，占大头） */}
            <div className='flex min-w-0 flex-[3] flex-col gap-1'>
              <div className='flex items-baseline justify-between'>
                <span className='text-sm font-medium'>{t('上游地址')}</span>
                <span className='text-xs text-text-3'>{t('每行一个')}</span>
              </div>
              <Textarea
                value={ipText}
                onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setIpText(event.target.value)}
                rows={9}
                className='font-mono text-xs'
                placeholder={'1.2.3.4\n5.6.7.8'}
                aria-label={t('上游地址')}
              />
              <div className='text-xs text-text-3'>
                {t('{n} 个地址', { n: parsedIps.ips.length })}
                {parsedIps.invalid.length > 0 && (
                  <span className='text-destructive'>
                    {' · '}
                    {t('{n} 行不合法', { n: parsedIps.invalid.length })}
                  </span>
                )}
                {draft.currentIp && ` · ${t('当前')} ${draft.currentIp}`}
              </div>
            </div>

            {/* 右：规则（紧凑一列） */}
            <div className='flex flex-[2] flex-col gap-4'>
              <div className='flex flex-col gap-1'>
                <span className='text-sm font-medium'>{t('等待响应超时')}</span>
                <div className='flex items-center gap-1'>
                  <Input
                    value={timeoutText}
                    onChange={(event: React.ChangeEvent<HTMLInputElement>) => setTimeoutText(event.target.value)}
                    inputMode='numeric'
                    className='w-20'
                    aria-label={t('等待响应超时')}
                  />
                  <span className='text-xs text-text-3'>{t('秒')}</span>
                </div>
                <span className='text-xs text-text-3'>
                  {t('0 = 跟随全局（{n} 秒）', { n: globalSeconds })}
                </span>
              </div>

              <div className='flex flex-col gap-1'>
                <span className='text-sm font-medium'>{t('异常响应状态码')}</span>
                <Input
                  value={codeText}
                  onChange={(event: React.ChangeEvent<HTMLInputElement>) => setCodeText(event.target.value)}
                  placeholder='502, 504'
                  className='font-mono'
                  aria-label={t('异常响应状态码')}
                />
                <span className='text-xs text-text-3'>{t('英文逗号分隔')}</span>
              </div>

              <div className='mt-auto rounded-md bg-surface-inset p-2 text-xs leading-relaxed text-text-2'>
                {t('说明：请求异常响应的状态码命中上面任意状态码就会触发切换上游 ip 和账号再次访问')}
              </div>
            </div>
          </div>
        </DialogBody>
        <DialogFooter>
          {status}
          <div className='mr-auto' />
          <Button variant='ghost' onClick={onClose} disabled={saving}>{t('取消')}</Button>
          <Button onClick={() => void save()} disabled={saving}>
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/** 把多行文本拆成 IP 列表：去空白、去空行、去重（保序 —— 顺序就是轮换顺序）。 */
function parseIps(text: string): { ips: string[]; invalid: string[] } {
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

/** 切换状态码：英文逗号分隔，100–599，去重排序。 */
function parseCodes(text: string): { codes: number[]; invalid: string[] } {
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

/** 点分十进制 IPv4 校验（与后端 `parse_ips` 同一口径，不含端口与 IPv6）。 */
function isIpv4(text: string): boolean {
  const parts = text.split('.')
  if (parts.length !== 4) return false
  return parts.every(part => /^\d{1,3}$/.test(part) && Number(part) <= 255)
}
