import { useEffect, useMemo, useState } from 'react'
import { PageHead, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { AccountSheet } from './AccountSheet'
import { PinnedUpstreamSheet } from './PinnedUpstreamSheet'
import { useStore, store } from '../state/store'
import {
  getPinnedUpstream,
  savePinnedUpstream,
  setAccountEnabled,
  type Account,
  type PinnedUpstreamSettings,
} from '../api/gateway'
import { accountName, formatExpiry, formatRelative } from '../lib/format'
import { expiryOf, hasExpiry } from '../lib/providerFacts'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 账号。
 *
 * ── 为什么是分组卡片而不是表格 ────────────────────────────────
 * 桌面端账号表有 10 列（顺序 / 账号 / 代理 / 连接数 / 状态 / 限流 / 有效期 /
 * 余额 / 操作…），压不进 390px。手机上只留五个关键读数：邮箱、状态、有效期、
 * 余额、代理；提供商信息上移到分组头，行里不再重复。
 *
 * ── 分组口径 ──────────────────────────────────────────────────
 * 与后端一致：同一提供商的账号是一组，组内按 priority 升序（= 转发尝试顺序）。
 * 组本身的先后也按最小 priority 排 —— 这就是全局队列在手机上的投影。
 *
 * ── 组的设置入口放哪 ──────────────────────────────────────────
 * 桌面端把「固定上游 IP」挂在组标题的 ⋯ 菜单里；手机没有菜单，所以直接给
 * 组头右侧两个入口：开着一枚「当前 IP + 切换」的读数（最常用的动作一步可达），
 * 加一个齿轮进设置抽屉。只有 WorkBuddy 国际版这一组有 —— 别家没有这个概念。
 */

type Filter = 'all' | 'enabled' | 'limited' | 'disabled'

export function AccountsPage() {
  const state = useStore()
  const [filter, setFilter] = useState<Filter>('all')
  const [query, setQuery] = useState('')
  const [openId, setOpenId] = useState<string | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  // 固定上游 IP：组头那枚读数 + 设置抽屉共用这一份设置
  const [pin, setPin] = useState<PinnedUpstreamSettings | null>(null)
  const [pinOpen, setPinOpen] = useState(false)
  const [rotating, setRotating] = useState(false)
  const accounts = state.session?.accounts?.accounts || []

  const loadPin = async (): Promise<void> => {
    try {
      const payload = await getPinnedUpstream()
      setPin(payload.pinnedUpstreamIp)
    } catch {
      // 读不到就只留齿轮入口（抽屉里会给出具体错误）：不能让一个次要读数
      // 把整页的错误条点亮
    }
  }

  // 只在进页面时读一次；之后的变更由抽屉保存 / 切换按钮各自刷新
  useEffect(() => {
    void loadPin()
  }, [])

  /** 手动前进一个 IP（组策略）：只影响之后的请求，不动正在跑的连接。 */
  const rotatePin = async (): Promise<void> => {
    if (!pin || rotating) return
    if (pin.ips.length < 2) {
      store.toast(t('只有一条 IP，无法切换'), 'err')
      return
    }
    setRotating(true)
    try {
      const index = pin.ips.indexOf(pin.currentIp)
      const next = pin.ips[(index + 1) % pin.ips.length]
      const payload = await savePinnedUpstream({ currentIp: next })
      setPin(payload.pinnedUpstreamIp)
      store.toast(t('已切换到上游 IP {ip}', { ip: next }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setRotating(false)
    }
  }

  usePolling(async () => {
    try {
      await store.refresh(true)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }, 30_000)

  const filtered = useMemo(() => {
    const keyword = query.trim().toLowerCase()
    return accounts.filter(item => {
      const limited = hasRateLimit(item)
      if (filter === 'enabled' && !item.enabled) return false
      if (filter === 'disabled' && item.enabled) return false
      if (filter === 'limited' && !limited) return false
      if (!keyword) return true
      const haystack = [item.name, item.nickname, item.email, item.id, item.provider]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return haystack.includes(keyword)
    })
  }, [accounts, filter, query])

  const groups = useMemo(() => {
    const map = new Map<string, Account[]>()
    filtered.forEach(item => {
      const provider = String(item.provider || 'unknown')
      const list = map.get(provider) || []
      list.push(item)
      map.set(provider, list)
    })
    return [...map.entries()]
      .map(([provider, list]) => ({
        provider,
        list: [...list].sort((a, b) => (a.priority ?? 999) - (b.priority ?? 999)),
      }))
      .sort((a, b) => (a.list[0]?.priority ?? 999) - (b.list[0]?.priority ?? 999))
  }, [filtered])

  const enabledCount = accounts.filter(item => item.enabled).length
  const limitedCount = accounts.filter(item => item.enabled && hasRateLimit(item)).length
  const disabledCount = accounts.filter(item => !item.enabled).length

  const toggle = async (account: Account, next: boolean) => {
    setBusy(true)
    try {
      await setAccountEnabled(account.id, next)
      store.toast(next ? t('已启用 {name}', { name: accountName(account) }) : t('已禁用 {name}', { name: accountName(account) }))
      await store.refresh(true)
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy(false)
    }
  }

  const openAccount = openId ? accounts.find(item => item.id === openId) || null : null

  return (
    <div className='m-page'>
      <PageHead
        title={t('账号')}
        subtitle={t('{n} 个账号 · {e} 启用 · {l} 限流', { n: accounts.length, e: enabledCount, l: limitedCount })}
        actions={
          <IconButton
            name='refresh'
            label={t('刷新')}
            onClick={() => {
              void store.refresh(true)
              store.toast(t('已刷新'))
            }}
          />
        }
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            height: 38,
            padding: '0 12px',
            borderRadius: 11,
            background: 'var(--m-surface)',
            border: '1px solid var(--m-line)',
            color: 'var(--m-text-3)',
          }}
        >
          <Icon name='search' style={{ width: 16, height: 16, flex: 'none' }} />
          <input
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('搜索账号 / 邮箱 / 备注')}
            style={{
              flex: 1,
              minWidth: 0,
              border: 0,
              outline: 'none',
              background: 'transparent',
              color: 'var(--m-text)',
              fontSize: 16,
            }}
          />
          {query ? (
            <button type='button' onClick={() => setQuery('')} aria-label={t('清除')} style={{ display: 'flex', border: 0, background: 'none', color: 'var(--m-text-3)' }}>
              <Icon name='close' style={{ width: 15, height: 15 }} />
            </button>
          ) : null}
        </div>
      </div>

      <div className='m-chips' style={{ padding: '10px 16px 12px' }}>
        <FilterChip label={t('全部')} count={accounts.length} on={filter === 'all'} onClick={() => setFilter('all')} />
        <FilterChip label={t('启用')} count={enabledCount} on={filter === 'enabled'} onClick={() => setFilter('enabled')} />
        <FilterChip label={t('限流')} count={limitedCount} on={filter === 'limited'} onClick={() => setFilter('limited')} />
        <FilterChip label={t('禁用')} count={disabledCount} on={filter === 'disabled'} onClick={() => setFilter('disabled')} />
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {!state.session && state.loading ? <SkeletonList rows={4} height={72} /> : null}

      {state.session && !groups.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty text={query ? t('没有匹配「{kw}」的账号', { kw: query }) : t('还没有账号，去桌面端添加')} />
          </div>
        </div>
      ) : null}

      {groups.map(group => {
        const enabled = group.list.filter(item => item.enabled).length
        const limited = group.list.filter(item => item.enabled && hasRateLimit(item)).length
        return (
          <div key={group.provider} className='m-section' style={{ marginTop: 12 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 9, padding: '0 3px 8px' }}>
              <ProviderLogo id={group.provider} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13, fontWeight: 600 }}>{providerLabel(group.provider)}</div>
                <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2 }}>
                  {t('{n} 个账号 · 启用 {e}', { n: group.list.length, e: enabled })}
                  {limited ? t(' · 限流 {n}', { n: limited }) : ''}
                </div>
              </div>
              {group.provider === PINNED_PROVIDER ? (
                <div style={{ display: 'flex', alignItems: 'center', gap: 4, flex: 'none' }}>
                  {pin?.enabled && pin.currentIp ? (
                    <span className='m-pin' title={t('固定上游 IP：组内「跟随组」的账号都用这个地址访问上游')}>
                      <span className='m-pin-ip'>{pin.currentIp}</span>
                      <button
                        type='button'
                        className='m-pin-rotate'
                        disabled={rotating}
                        aria-label={t('切换上游 IP')}
                        title={t('手动切换到下一个上游 IP（只影响之后的请求）')}
                        onClick={() => void rotatePin()}
                      >
                        <Icon name='refresh' />
                      </button>
                    </span>
                  ) : null}
                  <button
                    type='button'
                    className='m-pin-gear'
                    aria-label={t('固定上游 IP')}
                    title={t('固定上游 IP')}
                    onClick={() => setPinOpen(true)}
                  >
                    <Icon name='settings' />
                  </button>
                </div>
              ) : null}
            </div>
            <div className='m-card'>
              {group.list.map((account, index) => (
                <AccountRow
                  key={account.id}
                  account={account}
                  rank={index + 1}
                  onOpen={() => setOpenId(account.id)}
                  onToggle={next => void toggle(account, next)}
                  busy={busy}
                />
              ))}
            </div>
          </div>
        )
      })}

      <div className='m-section' style={{ marginTop: 16 }}>
        <div className='m-card'>
          <button
            type='button'
            className='m-row'
            onClick={() => store.toast(t('添加账号请用桌面端或网页版面板'))}
          >
            <span className='m-logo'>
              <Icon name='plus' />
            </span>
            <div className='m-grow'>
              <div className='m-t1'>{t('添加账号')}</div>
              <div className='m-t2'>{t('扫码 / 粘贴令牌 / 导入登录态需在桌面端完成')}</div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </button>
        </div>
      </div>

      <AccountSheet
        account={openAccount}
        onClose={() => setOpenId(null)}
        onChanged={() => void store.refresh(true)}
      />

      <PinnedUpstreamSheet
        open={pinOpen}
        onClose={() => setPinOpen(false)}
        onSaved={() => {
          // 保存可能重置了一批账号的独立 IP（列表变了的时候），所以账号列表
          // 也要重读 —— 不只是设置本身
          void loadPin()
          void store.refresh(true)
        }}
      />
    </div>
  )
}

function AccountRow({
  account,
  rank,
  onOpen,
  onToggle,
  busy,
}: {
  account: Account
  /** 组内序号（1 起）：后端 priority 是全局唯一队列里的绝对位次，手机上按组内序号读 */
  rank: number
  onOpen: () => void
  onToggle: (next: boolean) => void
  busy: boolean
}) {
  const limited = hasRateLimit(account)
  const expiresAt = expiryOf(account)
  const expired = expiresAt > 0 && expiresAt <= Date.now()
  const tone = !account.enabled
    ? 'mute'
    : account.needsRelogin
      ? 'bad'
      : limited
        ? 'warn'
        : expired
          ? 'bad'
          : 'ok'
  const badgeText = !account.enabled
    ? t('已禁用')
    : account.needsRelogin
      ? t('需重登')
      : limited
        ? t('限流')
        : expired
          ? t('已过期')
          : t('正常')
  // 有效期：这家没有这个概念时不显示，而不是显示「—」（少一行噪音）
  const expiryText = hasExpiry(account.provider) ? formatExpiry(expiresAt) : ''

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 11,
        padding: '12px 13px',
        borderBottom: '1px solid var(--m-line)',
        opacity: account.enabled ? 1 : 0.62,
      }}
    >
      <button
        type='button'
        onClick={onOpen}
        style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 11, background: 'none', border: 0, padding: 0, textAlign: 'left' }}
      >
        <span style={{ width: 20, flex: 'none', fontSize: 11, color: 'var(--m-text-3)', fontVariantNumeric: 'tabular-nums', textAlign: 'center' }}>
          #{rank}
        </span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 13.5, fontWeight: 550, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {accountName(account)}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 4, fontSize: 11.5, color: 'var(--m-text-3)' }}>
            <Badge tone={tone} dot>
              {badgeText}
            </Badge>
            {expiryText ? (
              <>
                <span>·</span>
                <span>{expiryText}</span>
              </>
            ) : null}
          </div>
        </div>
      </button>
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 5, flex: 'none' }}>
        <SwitchSmall checked={Boolean(account.enabled)} onChange={onToggle} disabled={busy} label={accountName(account)} />
        <span style={{ fontSize: 10.5, color: 'var(--m-text-3)' }}>
          {account.editionLabel || formatRelative(account.updatedAt as number)}
        </span>
      </div>
    </div>
  )
}

function FilterChip({
  label,
  count,
  on,
  onClick,
}: {
  label: string
  count: number
  on: boolean
  onClick: () => void
}) {
  return (
    <button type='button' className='m-chip' data-on={on} onClick={onClick}>
      {label}
      <span className='m-n'>{count}</span>
    </button>
  )
}

/** 小号开关：账号行里用它，尺寸比设置页的档位小一档 */
function SwitchSmall({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  label: string
}) {
  return (
    <button
      type='button'
      role='switch'
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        width: 40,
        height: 24,
        borderRadius: 999,
        border: checked ? '0' : '1px solid var(--m-line-strong)',
        background: checked ? 'var(--m-ok)' : 'var(--m-surface-3)',
        position: 'relative',
        transition: 'background .18s, border-color .18s',
        padding: 0,
        flex: 'none',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: checked ? 2 : 2,
          left: checked ? 18 : 2,
          width: 20,
          height: 20,
          borderRadius: '50%',
          background: '#fff',
          boxShadow: '0 1px 3px rgba(0,0,0,.25)',
          transition: 'left .18s cubic-bezier(.3,.9,.4,1)',
        }}
      />
    </button>
  )
}

/** 固定上游 IP 只作用于这一家（见 ADR-0001：国内版另有 growth 域，未评估） */
const PINNED_PROVIDER = 'workbuddy-intl'

function hasRateLimit(account: { rateLimits?: Record<string, unknown> }): boolean {
  return Boolean(account.rateLimits && Object.keys(account.rateLimits).length)
}
