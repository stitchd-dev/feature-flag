/**
 * ExperimentDetail — page-level smoke tests.
 *
 * The admin Vitest setup uses `environment: 'node'` (no DOM, no
 * `@testing-library/react`). This matches the rest of the page-level test
 * suite which exercises logic + module shape only — see
 * `EventDetail.test.ts`, `PreviewTab.test.ts`, etc.
 *
 * Coverage goal for Task 8.2:
 *   1. The page module no longer references the `EXPERIMENTS` mock — neither
 *      via import nor by symbol reference. This is the acceptance criterion
 *      for the Phase 8 cutover.
 *   2. The page imports the typed API wrappers `getExperiment` +
 *      `getExperimentResults` (Phase 8.1 contracts).
 *   3. The page imports `buildExperimentDisplay` from the helper module so
 *      derivations run through the unit-tested folder.
 *   4. (Task 8.3) The page wraps its body with `ContextTypeProvider` and
 *      renders the `ContextTypeTabs` primitive — verified by import shape.
 *
 * Phase 9 will add interaction tests when (and if) jsdom + Testing Library
 * are introduced — until then, the helper layer is the testable seam and
 * `ExperimentDetail.helpers.test.ts` already covers every derivation branch.
 */
import { describe, it, expect } from 'vitest'
// Vite raw-import: pulls the source file in as a literal string at build
// time. Avoids pulling in node:fs/path types from `@types/node` (which is
// not in `tsconfig.app.json`'s `types` array).
import SOURCE from './ExperimentDetail.tsx?raw'

describe('ExperimentDetail (page module)', () => {
  it('does not import the EXPERIMENTS mock', () => {
    // Negative grep: no import of EXPERIMENTS or the legacy Experiment type
    // from mockData. (Phase 8 cutover acceptance criterion.)
    expect(SOURCE).not.toMatch(/from\s+['"][^'"]*mockData['"]/)
    expect(SOURCE).not.toMatch(/\bEXPERIMENTS\b/)
  })

  it('imports the typed experiment API wrappers from lib/api', () => {
    expect(SOURCE).toMatch(/getExperiment\b/)
    expect(SOURCE).toMatch(/getExperimentResults\b/)
    expect(SOURCE).toMatch(/from\s+['"][^'"]*lib\/api['"]/)
  })

  it('imports buildExperimentDisplay from the helpers module', () => {
    expect(SOURCE).toMatch(/buildExperimentDisplay/)
    expect(SOURCE).toMatch(/from\s+['"]\.\/ExperimentDetail\.helpers['"]/)
  })

  it('uses Promise.all to load the experiment + results concurrently', () => {
    expect(SOURCE).toMatch(/Promise\.all/)
  })

  it('wraps the body with ContextTypeProvider (Task 8.3)', () => {
    expect(SOURCE).toMatch(/ContextTypeProvider/)
    expect(SOURCE).toMatch(/useActiveContextType/)
  })

  it('renders the ContextTypeTabs primitive (Task 8.3)', () => {
    expect(SOURCE).toMatch(/ContextTypeTabs/)
  })
})

describe('ExperimentDetail — lifecycle transitions (experiment_lifecycle_ui_20260610)', () => {
  it('wires the real LifecycleActions control', () => {
    expect(SOURCE).toMatch(/LifecycleActions/)
  })

  it('removed the dead Stop / Ship-winner header buttons', () => {
    expect(SOURCE).not.toMatch(/Ship winner/)
    expect(SOURCE).not.toMatch(/>\s*Stop\s*</)
  })

  it('derives the lifecycle timeline from real timestamps (no mock actors/dates)', () => {
    expect(SOURCE).toMatch(/lifecycleTimeline/)
    expect(SOURCE).not.toMatch(/Marco G\./)
    expect(SOURCE).not.toMatch(/Priya R\./)
  })

  it('no longer fabricates allocation / MDE / CUPED config values', () => {
    expect(SOURCE).not.toMatch(/380,000/)
    expect(SOURCE).not.toMatch(/variance reduction 18%/)
    expect(SOURCE).not.toMatch(/beta-customers AND country/)
  })
})
