// 原型入口（一次性，勿并入正式代码）
//
// 三个模态框变体挂在账号页上，用 ?variant=A|B|C 切换，底部浮动条切换与预览。
//
// 怎么用：
//   1. 先启动网关（见原型说明里的命令）
//   2. 浏览器打开 http://127.0.0.1:3999/?variant=A
//   3. 底部那条不显眼的浮动条点 ← → 切换，或按键盘左右方向键
//
// 关键：读的是真实的 /api/pinned-upstream-ip，保存也真的落库。
// 所以三个变体除了「长什么样」之外，行为必须一致 —— 这样你比较的才是布局。

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { VARIANTS, type BridgeWindow, type PinnedPayload, type VariantKey } from './pinned-upstream-modal.prototype'
// 原型样式（组件库 + Tailwind），由 vite.prototype.config.ts 抽成 ui/prototype/prototype.css
import './prototype.css'

const VARIANTS_META = VARIANTS

function currentVariant(): VariantKey {
  const value = new URLSearchParams(window.location.search).get('variant')
  const key = (value ?? 'A').toUpperCase()
  return (key in VARIANTS_META ? key : 'A') as VariantKey
}

function setVariant(key: VariantKey) {
  const url = new URL(window.location.href)
  url.searchParams.set('variant', key)
  window.history.replaceState(null, '', url)
  window.dispatchEvent(new PopStateEvent('popstate'))
}

/** 通过后端桥读当前设置（桌面壳与浏览器形态都走 api_request）。 */
async function loadSettings(): Promise<PinnedPayload | null> {
  const bridge = (window as BridgeWindow).__TAURI_INTERNALS__
  if (!bridge?.invoke) {
    throw new Error('这个页面没有注入桥接（window.__TAURI_INTERNALS__ 不存在）')
  }
  const response = await bridge.invoke('api_request', {
    request: { method: 'GET', path: '/api/pinned-upstream-ip', body: null },
  })
  return response as PinnedPayload
}

function Switcher({ current }: { current: VariantKey }) {
  return (
    <div className='fixed bottom-5 left-1/2 z-[9999] -translate-x-1/2 rounded-full bg-neutral-900 px-2 py-1.5 text-white shadow-lg'>
      <div className='flex items-center gap-2 text-xs'>
        <button
          type='button'
          className='rounded-full px-2 py-0.5 hover:bg-white/15'
          onClick={() => cycle(current, -1)}
          aria-label='上一个变体'
        >
          ←
        </button>
        <span className='min-w-[9rem] text-center'>
          {current}（{VARIANTS_META[current].label}）
        </span>
        <button
          type='button'
          className='rounded-full px-2 py-0.5 hover:bg-white/15'
          onClick={() => cycle(current, 1)}
          aria-label='下一个变体'
        >
          →
        </button>
      </div>
    </div>
  )
}

function cycle(current: VariantKey, direction: 1 | -1) {
  const keys = Object.keys(VARIANTS_META) as VariantKey[]
  const index = keys.indexOf(current)
  const next = keys[(index + direction + keys.length) % keys.length]
  setVariant(next)
}

function PrototypeHost() {
  const variant = currentVariant()
  const [payload, setPayload] = React.useState<PinnedPayload | null>(null)
  const [open, setOpen] = React.useState(true)
  const [loaded, setLoaded] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)

  React.useEffect(() => {
    loadSettings()
      .then(result => setPayload(result))
      .catch(reason => setError(String(reason)))
      .finally(() => setLoaded(true))
  }, [])

  // 键盘左右切换（输入框聚焦时不拦截 —— 否则在文本域里按方向键会跳变体）
  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null
      if (
        target &&
        (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)
      ) {
        return
      }
      if (event.key === 'ArrowLeft') cycle(currentVariant(), -1)
      if (event.key === 'ArrowRight') cycle(currentVariant(), 1)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  if (!loaded) {
    return <div className='fixed inset-0 grid place-items-center text-sm text-neutral-500'>读取配置中…</div>
  }
  if (error) {
    return (
      <div className='fixed inset-0 grid place-items-center px-6 text-center text-sm text-neutral-600'>
        <div>
          <div className='mb-2 font-medium text-neutral-800'>读取配置失败</div>
          <pre className='max-w-lg whitespace-pre-wrap rounded bg-neutral-100 p-3 text-left text-[11px]'>{error}</pre>
          <div className='mt-2 text-xs'>桥接类型：{typeof (window as BridgeWindow).__TAURI_INTERNALS__}</div>
        </div>
      </div>
    )
  }
  if (!payload || !payload.pinnedUpstreamIp) {
    return (
      <div className='fixed inset-0 grid place-items-center px-6 text-center text-sm text-neutral-600'>
        <div>
          <div className='mb-2 font-medium text-neutral-800'>读不到固定上游 IP 的配置</div>
          <div className='max-w-lg text-xs leading-relaxed'>
            常见原因：网关没起来、页面不是从网关打开的（没有注入桥接）、
            或者后端还没带上这个接口。
          </div>
          <pre className='mt-3 max-h-64 overflow-auto rounded bg-neutral-100 p-3 text-left text-[11px]'>
            {JSON.stringify({ payload, bridge: typeof (window as BridgeWindow).__TAURI_INTERNALS__ }, null, 2)}
          </pre>
        </div>
      </div>
    )
  }

  const Variant = VARIANTS_META[variant].component
  const settings = payload.pinnedUpstreamIp

  return (
    <>
      {!open && (
        <div className='fixed inset-0 grid place-items-center bg-neutral-100'>
          <div className='flex flex-col items-center gap-3'>
            <div className='text-sm text-neutral-600'>变体 {variant} 的弹窗已关闭</div>
            <button
              type='button'
              className='rounded-md bg-neutral-900 px-3 py-1.5 text-xs text-white'
              onClick={() => setOpen(true)}
            >
              重新打开弹窗
            </button>
            <div className='max-w-md whitespace-pre-wrap rounded-md bg-white p-3 font-mono text-[11px] text-neutral-600 shadow'>
              {JSON.stringify(payload, null, 2)}
            </div>
          </div>
        </div>
      )}
      {open && (
        <Variant
          key={variant}
          settings={settings}
          globalWaitTimeoutSeconds={payload.globalWaitTimeoutSeconds}
          onClose={() => setOpen(false)}
          onSaved={saved => {
            setPayload(saved)
            setOpen(false)
          }}
        />
      )}
      <Switcher current={variant} />
    </>
  )
}

/** 把原型挂到一个宿主容器里（挂一次就够了，切换变体不重挂）。 */
class PrototypeBoundary extends React.Component<
  { children: React.ReactNode },
  { message: string | null }
> {
  state = { message: null as string | null }
  static getDerivedStateFromError(error: unknown) {
    return { message: String(error) }
  }
  componentDidCatch(error: unknown) {
    // 控制台也留一份：截图看不到时能从这里捞
    console.error('[prototype] 渲染失败', error)
  }
  render() {
    if (this.state.message) {
      return (
        <div style={{ position: 'fixed', inset: 0, display: 'grid', placeItems: 'center', padding: 24 }}>
          <div>
            <div style={{ fontWeight: 600, marginBottom: 8 }}>原型渲染失败</div>
            <pre style={{ maxWidth: 640, whiteSpace: 'pre-wrap', background: '#f4f4f5', padding: 12, fontSize: 11 }}>
              {this.state.message}
            </pre>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}

function mount(): boolean {
  if (document.querySelector('[data-prototype-host]')) return true
  const element = document.createElement('div')
  element.setAttribute('data-prototype-host', '')
  document.body.append(element)
  createRoot(element).render(
    <PrototypeBoundary>
      <PrototypeHost />
    </PrototypeBoundary>,
  )
  return true
}

// 原型直接挂 body，而不是等账号页渲染出来再挂进它的 section：
// 账号页是异步渲染的（岛要等桥接就绪），而弹窗本身是 fixed 定位、盖住整页，
// 挂在哪儿都盖得住 —— 挂在 body 上就不依赖那个 page 何时出现。
//
// 但**先把页面切到账号页**：弹窗背后的背景应当是有内容的账号表（原型要贴着一页
// 真实的、有密度的界面来评，正是不该放在白纸上的理由）。
function showAccountsPage() {
  const app = (window as unknown as { wbApp?: { showPage?: (name: string) => void } }).wbApp
  app?.showPage?.('accounts')
}

showAccountsPage()
let attempts = 0
const timer = window.setInterval(() => {
  attempts += 1
  if (attempts === 4) showAccountsPage()
  if (mount() || attempts > 40) {
    window.clearInterval(timer)
  }
}, 500)
