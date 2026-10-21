import { useEffect, useState } from 'react'
import { Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle, Switch } from '@ui'
import { store } from '../state/store'
import { getPinnedUpstream, savePinnedUpstream } from '../api/gateway'
import { t } from '../i18n'

/**
 * 固定上游 IP 的组级设置（账号页 WorkBuddy 国际版组头的齿轮进入）。
 *
 * ── 为什么手机上仍然给「配置」而不是只给读数 ──────────────────
 * 桌面端把这个功能藏在组菜单里，手机端没有组菜单；而它恰恰是**出门在外最
 * 可能要用**的一类设置（这条线路在酒店 / 4G 下不通，换一个 IP 就好）。
 * 只读的读数会让用户必须回到电脑前改一个数字。
 *
 * 形态仍是「一个底部抽屉、四样东西」：开关、地址（多行）、超时、状态码。
 * 没有照搬桌面端的左右分栏 —— 390px 下两列会把地址压成一条缝。
 *
 * ── 保存时后端会做的事（要和用户说清楚）──────────────────────
 *   · 开关打开但一个 IP 都没有 → 后端 400，这里先在前端拦一次；
 *   · **IP 列表一变**，该组所有账号的独立 IP 会被重置为「跟随组」，组当前
 *     IP 也会从新列表里重选。这是改完列表后最容易困惑的一点，保存成功后
 *     用 toast 明说（与桌面端同一句文案）。
 *
 * 术语见 GLOSSARY.md：固定上游 IP / 组策略 / 账号策略 / 异常响应状态码。
 */
export function PinnedUpstreamSheet({
  open,
  onClose,
  onSaved,
}: {
  open: boolean
  onClose: () => void
  /** 保存成功：让账号页重读设置（组头读数与账号下拉都依赖它） */
  onSaved: () => void
}) {
  const [loading, setLoading] = useState(true)
  const [enabled, setEnabled] = useState(false)
  const [ipText, setIpText] = useState('')
  const [timeoutText, setTimeoutText] = useState('0')
  const [codeText, setCodeText] = useState('502')
  const [globalSeconds, setGlobalSeconds] = useState(300)
  const [currentIp, setCurrentIp] = useState('')
  const [openedIps, setOpenedIps] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  // 每次打开都重读：组头那枚「切换」按钮改的是同一个 currentIp，不重读会
  // 拿着打开前的旧快照去覆盖它
  useEffect(() => {
    if (!open) return
    let cancelled = false
    setLoading(true)
    setError('')
    void (async () => {
      try {
        const payload = await getPinnedUpstream()
        if (cancelled) return
        const settings = payload.pinnedUpstreamIp
        setEnabled(settings.enabled)
        setIpText(settings.ips.join('\n'))
        setTimeoutText(String(settings.waitTimeoutSeconds))
        setCodeText(settings.statusCodes.join(', '))
        setCurrentIp(settings.currentIp)
        setGlobalSeconds(payload.globalWaitTimeoutSeconds)
        setOpenedIps(settings.ips.join(','))
      } catch (failure) {
        if (!cancelled) setError(failure instanceof Error ? failure.message : String(failure))
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [open])

  const parsedIps = parseIps(ipText)
  const parsedCodes = parseCodes(codeText)

  const save = async (): Promise<void> => {
    if (saving) return
    if (enabled && parsedIps.ips.length === 0) {
      setError(t('已开启，请至少填写一个 IP 地址'))
      return
    }
    if (parsedIps.invalid.length) {
      setError(t('不是合法 IPv4：{list}', { list: parsedIps.invalid.join('、') }))
      return
    }
    if (parsedCodes.invalid.length) {
      setError(t('状态码不合法：{list}', { list: parsedCodes.invalid.join('、') }))
      return
    }
    setSaving(true)
    setError('')
    try {
      await savePinnedUpstream({
        enabled,
        ips: parsedIps.ips,
        // 空串 / 非数字都收成 0（= 跟随全局）：后端只认 0 或范围内的整数，
        // 传 NaN 会被它按「必须是整数秒」拒掉，而这里空着是用户最常见的输入
        waitTimeoutSeconds: Number(timeoutText) || 0,
        statusCodes: parsedCodes.codes,
      })
      const ipsChanged = parsedIps.ips.join(',') !== openedIps
      store.toast(
        ipsChanged
          ? t('已保存：IP 列表变了，组内账号的独立 IP 已重置为「跟随组」')
          : t('固定上游 IP 设置已保存'),
      )
      onSaved()
      onClose()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Sheet open={open} onOpenChange={next => { if (!next && !saving) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('固定上游 IP')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          {loading ? (
            <div style={{ fontSize: 13, color: 'var(--m-text-3)' }}>{t('读取中…')}</div>
          ) : (
            <>
              <div className='m-row' style={{ paddingLeft: 0, paddingRight: 0 }}>
                <div className='m-grow'>
                  <div className='m-t1'>{t('启用固定上游 IP')}</div>
                  <div className='m-t2'>
                    {enabled ? t('已开启') : t('已关闭')}
                  </div>
                </div>
                <Switch
                  checked={enabled}
                  onCheckedChange={setEnabled}
                  aria-label={t('启用固定上游 IP')}
                />
              </div>

              <div className='m-field'>
                <div className='m-field-head'>
                  <span className='m-field-label'>{t('上游地址')}</span>
                  <span className='m-field-hint'>{t('每行一个')}</span>
                </div>
                <textarea
                  value={ipText}
                  onChange={event => setIpText(event.target.value)}
                  rows={6}
                  spellCheck={false}
                  autoCapitalize='off'
                  autoCorrect='off'
                  placeholder={'1.2.3.4\n5.6.7.8'}
                  aria-label={t('上游地址')}
                  style={{ ...fieldStyle, height: 'auto', minHeight: 104, padding: '9px 12px', fontFamily: 'var(--m-mono)', fontSize: 16, lineHeight: 1.5, resize: 'vertical' }}
                />
                <div className='m-field-note'>
                  {t('{n} 个地址', { n: parsedIps.ips.length })}
                  {parsedIps.invalid.length > 0
                    ? ` · ${t('{n} 行不合法', { n: parsedIps.invalid.length })}`
                    : ''}
                  {currentIp ? ` · ${t('当前')} ${currentIp}` : ''}
                </div>
              </div>

              <div className='m-field'>
                <div className='m-field-head'>
                  <span className='m-field-label'>{t('等待响应超时')}</span>
                  <span className='m-field-hint'>{t('秒')}</span>
                </div>
                <input
                  value={timeoutText}
                  onChange={event => setTimeoutText(event.target.value)}
                  inputMode='numeric'
                  aria-label={t('等待响应超时')}
                  style={fieldStyle}
                />
                <div className='m-field-note'>{t('0 = 跟随全局（{n} 秒）', { n: globalSeconds })}</div>
              </div>

              <div className='m-field'>
                <div className='m-field-head'>
                  <span className='m-field-label'>{t('异常响应状态码')}</span>
                  <span className='m-field-hint'>{t('英文逗号分隔')}</span>
                </div>
                <input
                  value={codeText}
                  onChange={event => setCodeText(event.target.value)}
                  inputMode='numeric'
                  placeholder='502, 504'
                  aria-label={t('异常响应状态码')}
                  style={{ ...fieldStyle, fontFamily: 'var(--m-mono)' }}
                />
              </div>

              <div className='m-field-note' style={{ background: 'var(--m-surface-2)', borderRadius: 11, padding: '10px 12px', lineHeight: 1.6 }}>
                {t('说明：请求异常响应的状态码命中上面任意状态码就会触发切换上游 ip 和账号再次访问')}
              </div>

              {error ? (
                <div style={{ fontSize: 12.5, color: 'var(--m-danger)', lineHeight: 1.5 }}>❌ {error}</div>
              ) : null}
            </>
          )}
        </SheetBody>
        <SheetFooter style={{ flexDirection: 'column', alignItems: 'stretch', gap: 8 }}>
          <div className='m-btn-row'>
            <button
              type='button'
              className='m-btn m-ghost'
              style={{ flex: 1 }}
              disabled={saving}
              onClick={onClose}
            >
              {t('取消')}
            </button>
            <button
              type='button'
              className='m-btn'
              style={{ flex: 1 }}
              disabled={saving || loading}
              onClick={() => void save()}
            >
              {saving ? t('保存中…') : t('保存')}
            </button>
          </div>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

/** 输入框的统一外观：16px 是 iOS 的聚焦阈值（小于它 Safari 会把整页放大）。 */
const fieldStyle: React.CSSProperties = {
  width: '100%',
  height: 40,
  padding: '0 12px',
  borderRadius: 11,
  border: '1px solid var(--m-line)',
  background: 'var(--m-surface-2)',
  color: 'var(--m-text)',
  fontSize: 16,
  outline: 'none',
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

/** 异常响应状态码：英文逗号分隔、100–599、去重排序。 */
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

/** 点分十进制 IPv4 校验（与后端 `parse_ips` 同一口径：不含端口与 IPv6）。 */
function isIpv4(text: string): boolean {
  const parts = text.split('.')
  if (parts.length !== 4) return false
  return parts.every(part => /^\d{1,3}$/.test(part) && Number(part) <= 255)
}
