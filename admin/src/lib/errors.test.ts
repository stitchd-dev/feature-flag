import { describe, it, expect } from 'vitest'
import { extractErrorMessage } from './errors'

describe('extractErrorMessage', () => {
  it('extracts Axios response.data.message', () => {
    const err = { response: { data: { message: 'Not found' } } }
    expect(extractErrorMessage(err)).toBe('Not found')
  })

  it('falls back to err.message for plain Error', () => {
    const err = new Error('Network error')
    expect(extractErrorMessage(err)).toBe('Network error')
  })

  it('falls back to err.message for plain objects', () => {
    const err = { message: 'Something went wrong' }
    expect(extractErrorMessage(err)).toBe('Something went wrong')
  })

  it('falls back to String(err) for string errors', () => {
    expect(extractErrorMessage('just a string')).toBe('just a string')
  })

  it('falls back to String(err) for numbers', () => {
    expect(extractErrorMessage(42)).toBe('42')
  })

  it('handles null', () => {
    expect(extractErrorMessage(null)).toBe('An unknown error occurred')
  })

  it('handles undefined', () => {
    expect(extractErrorMessage(undefined)).toBe('An unknown error occurred')
  })

  it('skips empty response.data.message and falls back to err.message', () => {
    const err = { response: { data: { message: '' } }, message: 'Fallback message' }
    expect(extractErrorMessage(err)).toBe('Fallback message')
  })

  it('handles response.data without message field', () => {
    const err = { response: { data: { code: 404 } }, message: 'Not found' }
    expect(extractErrorMessage(err)).toBe('Not found')
  })

  it('handles nested axios error structure', () => {
    const err = {
      response: { data: { message: 'Unauthorized' } },
      message: 'Request failed with status code 401',
    }
    expect(extractErrorMessage(err)).toBe('Unauthorized')
  })
})
