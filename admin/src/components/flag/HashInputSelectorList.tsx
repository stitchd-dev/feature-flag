/**
 * HashInputSelectorList — the cross-context selector control for
 * percentage-allocation rule outputs and default-rule distributions.
 *
 * Phase 7 (flag_eval_unify_20260522) introduces this control as the
 * canonical authoring surface for `hash_inputs` (replacing the legacy
 * `hash_targets` map-shaped editor in `PercentageRolloutEditor`).
 *
 * UX contract:
 *
 *   • Ordered list of rows (`role="list"` / `role="listitem"`).
 *   • Each row picks: `context_type` (with autocomplete from observed
 *     context types) + field type (`Key` / `Parameter`) + parameter name
 *     (with autocomplete from observed parameters when field is `Parameter`).
 *   • Reorder via drag handle (HTML5 native DnD — no extra dep) or via
 *     keyboard (Alt+ArrowUp / Alt+ArrowDown when focused on a row).
 *   • Add a new row with the "Add hash input" button.
 *   • Remove a row with the per-row x button (with min-1 guard handled
 *     by the parent's Yup schema — UI keeps the button enabled but the
 *     resulting empty array fails validation on submit).
 *   • Live worked-example banner shows the resolved hash expression:
 *     `Bucket = hash(user.key || user.params.name || device.params.os)`.
 *
 * Validation surface:
 *
 *   The component is controlled — the parent owns the selector array and
 *   can pass `arrayError` (array-level Yup message) plus `rowErrors` (a
 *   map from row index to per-row Yup message). The component does not
 *   run Yup itself; the parent calls `hashInputsSchema.validateSync` and
 *   threads errors back in.
 *
 * Wire format:
 *
 *   Selectors mirror the gateway's `HashSelectorJson` 1:1 — the array can
 *   be assigned to `allocation.hash_inputs` without transformation. See
 *   `admin/src/lib/hashInputTypes.ts`.
 */
import { useEffect, useRef, useState } from 'react'
import { I } from '../icons'
import { SuggestionInput } from '../SuggestionInput'
import type { SuggestionItem } from '../SuggestionInput'
import {
  useContextParamSuggestions,
  useContextTypeSuggestions,
} from '../../hooks/useContextSuggestions'
import {
  formatSelector,
  type HashSelector,
} from '../../lib/hashInputTypes'
import {
  addSelector,
  formatWorkedExample,
  moveSelectorDown,
  moveSelectorUp,
  removeSelector,
} from './HashInputSelectorList.helpers'

// ─── Component ───────────────────────────────────────────────────────────────

interface Props {
  /** Ordered selector list. */
  value: HashSelector[]
  /** Called with the next selector list on every edit / reorder. */
  onChange: (next: HashSelector[]) => void
  /** Environment ID for context-type / param autocomplete. `null` disables autocomplete. */
  envId?: string | null
  /** Disable interaction (locked-by-experiment etc.). Renders rows read-only. */
  disabled?: boolean
  /** Array-level Yup error (e.g. "At least one hash input is required."). */
  arrayError?: string
  /** Map from row index → per-row Yup error. */
  rowErrors?: Record<number, string>
}

export function HashInputSelectorList({
  value,
  onChange,
  envId,
  disabled,
  arrayError,
  rowErrors,
}: Props) {
  return (
    <div data-testid="hash-input-selector-list">
      <div
        style={{
          fontSize: 11,
          fontWeight: 600,
          color: 'var(--fg-subtle)',
          textTransform: 'uppercase',
          letterSpacing: '0.06em',
          marginBottom: 6,
        }}
      >
        Hash by (determines bucket stickiness)
      </div>

      <ol
        role="list"
        aria-label="Hash inputs"
        style={{
          listStyle: 'none',
          padding: 0,
          margin: 0,
          display: 'flex',
          flexDirection: 'column',
          gap: 6,
        }}
      >
        {value.map((sel, i) => (
          <SelectorRow
            key={i}
            index={i}
            selector={sel}
            envId={envId ?? null}
            disabled={disabled ?? false}
            error={rowErrors?.[i]}
            isFirst={i === 0}
            isLast={i === value.length - 1}
            onPatch={(patch) =>
              onChange(
                value.map((s, j) =>
                  j === i ? ({ ...s, ...patch } as HashSelector) : s,
                ),
              )
            }
            onReplace={(replacement) =>
              onChange(value.map((s, j) => (j === i ? replacement : s)))
            }
            onRemove={() => onChange(removeSelector(value, i))}
            onMoveUp={() => onChange(moveSelectorUp(value, i))}
            onMoveDown={() => onChange(moveSelectorDown(value, i))}
            onDropAt={(from) => {
              if (from === i) return
              const next = [...value]
              const [moved] = next.splice(from, 1)
              next.splice(i, 0, moved)
              onChange(next)
            }}
          />
        ))}
      </ol>

      {arrayError && (
        <div
          role="alert"
          style={{
            fontSize: 12,
            color: 'var(--danger)',
            marginTop: 6,
          }}
        >
          {arrayError}
        </div>
      )}

      {!disabled && (
        <button
          type="button"
          className="btn sm"
          style={{ marginTop: 8 }}
          onClick={() => onChange(addSelector(value))}
        >
          <I.plus size={11} /> Add hash input
        </button>
      )}

      <WorkedExampleBanner value={value} />
    </div>
  )
}

// ─── SelectorRow ─────────────────────────────────────────────────────────────

interface RowProps {
  index: number
  selector: HashSelector
  envId: string | null
  disabled: boolean
  error?: string
  isFirst: boolean
  isLast: boolean
  onPatch: (patch: Partial<HashSelector>) => void
  onReplace: (next: HashSelector) => void
  onRemove: () => void
  onMoveUp: () => void
  onMoveDown: () => void
  onDropAt: (from: number) => void
}

function SelectorRow({
  index,
  selector,
  envId,
  disabled,
  error,
  isFirst,
  isLast,
  onPatch,
  onReplace,
  onRemove,
  onMoveUp,
  onMoveDown,
  onDropAt,
}: RowProps) {
  const rowRef = useRef<HTMLLIElement>(null)
  const [dragOver, setDragOver] = useState(false)

  // ── Context-type autocomplete (always loaded when envId is present). ──────
  const ctState = useContextTypeSuggestions(envId)
  const ctSuggestions: SuggestionItem[] = ctState.data.map((t) => ({
    label: t.context_type,
  }))

  // ── Parameter autocomplete (only when kind === 'context_parameter' and
  //    the row's context_type is non-empty). ──────────────────────────────────
  const paramCtxType =
    selector.kind === 'context_parameter' ? selector.context_type : null
  const paramState = useContextParamSuggestions(envId, paramCtxType || null)
  const paramSuggestions: SuggestionItem[] = paramState.data.map((p) => ({
    label: p.param_key,
    isPrivate: p.is_private,
  }))

  // ── Keyboard reorder (Alt+Up / Alt+Down). ─────────────────────────────────
  function handleKeyDown(e: React.KeyboardEvent<HTMLLIElement>) {
    if (!e.altKey) return
    if (e.key === 'ArrowUp' && !isFirst) {
      e.preventDefault()
      onMoveUp()
    } else if (e.key === 'ArrowDown' && !isLast) {
      e.preventDefault()
      onMoveDown()
    }
  }

  // ── Native HTML5 drag-and-drop. The dragHandle owns `draggable` so users
  //    don't accidentally drag while editing fields. ─────────────────────────
  function onDragStart(e: React.DragEvent) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', String(index))
  }

  function onDragOver(e: React.DragEvent) {
    e.preventDefault()
    e.dataTransfer.dropEffect = 'move'
    setDragOver(true)
  }

  function onDragLeave() {
    setDragOver(false)
  }

  function onDrop(e: React.DragEvent) {
    e.preventDefault()
    setDragOver(false)
    const from = parseInt(e.dataTransfer.getData('text/plain'), 10)
    if (!Number.isNaN(from)) onDropAt(from)
  }

  // ── Kind toggle: switching between ContextKey and ContextParameter has to
  //    REPLACE the whole selector because the shapes differ (parameter is
  //    only present on ContextParameter). ────────────────────────────────────
  function setKind(nextKind: 'context_key' | 'context_parameter') {
    if (selector.kind === nextKind) return
    if (nextKind === 'context_key') {
      onReplace({ kind: 'context_key', context_type: selector.context_type })
    } else {
      onReplace({
        kind: 'context_parameter',
        context_type: selector.context_type,
        parameter: '',
      })
    }
  }

  return (
    <li
      ref={rowRef}
      role="listitem"
      tabIndex={0}
      aria-label={`Hash input ${index + 1}: ${formatSelector(selector)}`}
      onKeyDown={handleKeyDown}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        padding: '4px 6px',
        background: dragOver ? 'var(--bg-sunken)' : 'transparent',
        borderRadius: 4,
        outline: dragOver ? '2px solid var(--accent)' : 'none',
        outlineOffset: -2,
      }}
    >
      {/* Drag handle owns `draggable` so input drags don't fire. */}
      <span
        aria-label={`Drag to reorder hash input ${index + 1}`}
        title="Drag to reorder (or Alt+↑ / Alt+↓ when focused)"
        draggable={!disabled}
        onDragStart={onDragStart}
        style={{
          cursor: disabled ? 'not-allowed' : 'grab',
          color: 'var(--fg-subtle)',
          flexShrink: 0,
        }}
      >
        <I.grip size={12} />
      </span>

      {/* context_type — combobox with autocomplete suggestions. */}
      <SuggestionInput
        value={selector.context_type}
        onChange={(v) => onPatch({ context_type: v })}
        suggestions={ctSuggestions}
        placeholder="context type"
        disabled={disabled}
        className="input"
        style={{ width: 110, fontFamily: 'var(--font-mono)', fontSize: 12 }}
      />

      <span style={{ fontSize: 11, color: 'var(--fg-subtle)' }}>.</span>

      {/* Field type: radio for ContextKey vs ContextParameter. */}
      <label
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 4,
          fontSize: 12,
          cursor: disabled ? 'not-allowed' : 'pointer',
          whiteSpace: 'nowrap',
        }}
      >
        <input
          type="radio"
          checked={selector.kind === 'context_key'}
          onChange={() => setKind('context_key')}
          disabled={disabled}
        />
        key
      </label>
      <label
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 4,
          fontSize: 12,
          cursor: disabled ? 'not-allowed' : 'pointer',
          whiteSpace: 'nowrap',
        }}
      >
        <input
          type="radio"
          checked={selector.kind === 'context_parameter'}
          onChange={() => setKind('context_parameter')}
          disabled={disabled}
        />
        param:
      </label>

      {selector.kind === 'context_parameter' && (
        <SuggestionInput
          value={selector.parameter}
          onChange={(v) =>
            onPatch({ parameter: v } as Partial<HashSelector>)
          }
          suggestions={paramSuggestions}
          placeholder="param name"
          disabled={disabled}
          className="input"
          style={{ width: 130, fontFamily: 'var(--font-mono)', fontSize: 12 }}
        />
      )}

      {/* Keyboard reorder buttons — also serve as a click-affordance for
          users who can't drag. */}
      <span style={{ display: 'inline-flex', gap: 2, marginLeft: 'auto' }}>
        <button
          type="button"
          className="icon-btn"
          aria-label={`Move hash input ${index + 1} up`}
          disabled={disabled || isFirst}
          onClick={onMoveUp}
          style={{ color: 'var(--fg-subtle)' }}
        >
          <I.chevronUp size={12} />
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-label={`Move hash input ${index + 1} down`}
          disabled={disabled || isLast}
          onClick={onMoveDown}
          style={{ color: 'var(--fg-subtle)' }}
        >
          <I.chevronDown size={12} />
        </button>
      </span>

      <button
        type="button"
        className="icon-btn"
        aria-label={`Remove hash input ${index + 1}`}
        disabled={disabled}
        onClick={onRemove}
        style={{ color: 'var(--danger)' }}
      >
        <I.x size={12} />
      </button>

      {error && (
        <span
          role="alert"
          style={{
            fontSize: 11,
            color: 'var(--danger)',
            marginLeft: 8,
          }}
        >
          {error}
        </span>
      )}
    </li>
  )
}

// ─── Worked-example banner ───────────────────────────────────────────────────

function WorkedExampleBanner({ value }: { value: HashSelector[] }) {
  // The banner re-renders on every value change — no memo needed for a
  // few-element string concat. Pure render.
  useExampleSync(value) // kept side-effect-free; future hooks can mount here

  return (
    <div
      data-testid="hash-input-worked-example"
      style={{
        marginTop: 10,
        padding: '8px 10px',
        background: 'var(--bg-sunken)',
        border: '1px solid var(--border-faint)',
        borderRadius: 6,
        fontFamily: 'var(--font-mono)',
        fontSize: 11,
        color: 'var(--fg-muted)',
      }}
    >
      {formatWorkedExample(value)}
    </div>
  )
}

/** Reserved for future hooks (e.g. telemetry pings on edit). Returns void. */
function useExampleSync(_value: HashSelector[]): void {
  useEffect(() => {
    // No-op for now. Kept as a hook anchor so we can wire telemetry /
    // analytics without restructuring the banner later.
  }, [_value])
}
