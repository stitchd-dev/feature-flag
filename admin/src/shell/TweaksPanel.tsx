import type { Tweaks } from '../hooks/useTweaks'
import { I } from '../components/icons'

interface Props {
  tweaks: Tweaks
  setTweak: <K extends keyof Tweaks>(key: K, value: Tweaks[K]) => void
  onClose: () => void
}

const ACCENTS = ['#F0461F', '#1F6FBF', '#3DD68C', '#A892FF', '#E6B14F', '#131418']

export function TweaksPanel({ tweaks, setTweak, onClose }: Props) {
  return (
    <div style={{ position: 'fixed', top: 16, right: 16, width: 280, background: 'var(--bg-elev)', border: '1px solid var(--border-strong)', borderRadius: 12, boxShadow: 'var(--shadow-lg)', zIndex: 200, fontFamily: 'var(--font-sans)' }}>
      <div style={{ padding: '12px 14px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <div style={{ fontFamily: 'var(--font-display)', fontWeight: 800, fontSize: 14 }}>Tweaks</div>
        <button onClick={onClose} className="button-reset icon-btn" style={{ color: 'var(--fg-muted)' }}><I.x size={14} /></button>
      </div>
      <div style={{ padding: 14, display: 'flex', flexDirection: 'column', gap: 14 }}>
        <Field label="Theme">
          <Seg value={tweaks.theme} onChange={(v) => setTweak('theme', v as Tweaks['theme'])} options={[['light', 'Light'], ['dark', 'Dark']]} />
        </Field>
        <Field label="Navigation">
          <Seg value={tweaks.navStyle} onChange={(v) => setTweak('navStyle', v as Tweaks['navStyle'])} options={[['sidebar', 'Sidebar'], ['rail', 'Rail'], ['topbar', 'Top bar']]} />
        </Field>
        <Field label="Flags page layout">
          <Seg value={tweaks.flagsLayout} onChange={(v) => setTweak('flagsLayout', v as Tweaks['flagsLayout'])} options={[['table', 'Table'], ['cards', 'Cards'], ['grouped', 'Grouped']]} />
        </Field>
        <Field label="Flag detail layout">
          <Seg value={tweaks.flagDetailLayout} onChange={(v) => setTweak('flagDetailLayout', v as Tweaks['flagDetailLayout'])} options={[['stacked', 'Stacked'], ['side', 'Side-by-side']]} />
        </Field>
        <Field label="Experiment viz">
          <Seg value={tweaks.expViz} onChange={(v) => setTweak('expViz', v as Tweaks['expViz'])} options={[['auto', 'Auto'], ['frequentist', 'Freq'], ['bayesian', 'Bayes']]} />
        </Field>
        <Field label="Density">
          <Seg value={tweaks.density} onChange={(v) => setTweak('density', v as Tweaks['density'])} options={[['comfortable', 'Cozy'], ['compact', 'Compact']]} />
        </Field>
        <Field label="Accent color">
          <div style={{ display: 'flex', gap: 6 }}>
            {ACCENTS.map((c) => (
              <button
                key={c}
                onClick={() => setTweak('accent', c)}
                style={{ width: 28, height: 28, borderRadius: 6, background: c, border: tweaks.accent === c ? '2px solid var(--fg)' : '1px solid var(--border)', cursor: 'pointer' }}
              />
            ))}
          </div>
        </Field>
      </div>
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div style={{ fontSize: 11, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.06em', color: 'var(--fg-muted)', marginBottom: 6 }}>{label}</div>
      {children}
    </div>
  )
}

function Seg({ value, onChange, options }: { value: string; onChange: (v: string) => void; options: [string, string][] }) {
  return (
    <div style={{ display: 'flex', gap: 4, padding: 3, background: 'var(--bg-sunken)', borderRadius: 6, border: '1px solid var(--border)' }}>
      {options.map(([k, l]) => (
        <button
          key={k}
          onClick={() => onChange(k)}
          style={{ flex: 1, padding: '5px 8px', border: 'none', borderRadius: 4, background: value === k ? 'var(--surface)' : 'transparent', color: value === k ? 'var(--fg)' : 'var(--fg-muted)', fontSize: 11, fontWeight: 500, cursor: 'pointer', boxShadow: value === k ? 'var(--shadow-xs)' : 'none' }}
        >
          {l}
        </button>
      ))}
    </div>
  )
}
