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
 *
 * Phase 9 will add interaction tests when (and if) jsdom + Testing Library
 * are introduced — until then, the helper layer is the testable seam and
 * `ExperimentDetail.helpers.test.ts` already covers every derivation branch.
 */
import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const SOURCE_PATH = resolve(__dirname, 'ExperimentDetail.tsx')
const SOURCE = readFileSync(SOURCE_PATH, 'utf8')

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

  it('reads context-type from localStorage with the documented key shape', () => {
    // Active-context-type is persisted under experiment_${key}_ctx (matches
    // the spec for Task 8.3, which lifts this into a provider).
    expect(SOURCE).toMatch(/experiment_.*_ctx/)
  })

  it('uses Promise.all to load the experiment + results concurrently', () => {
    expect(SOURCE).toMatch(/Promise\.all/)
  })
})
