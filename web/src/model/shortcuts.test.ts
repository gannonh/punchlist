import { describe, expect, test } from 'vitest'
import { PICKERS } from '@/components/bulk-menu'
import { keyAction, STRIPS, type StripScreen } from './shortcuts'

const screens = Object.keys(STRIPS) as StripScreen[]

describe('shortcut strips', () => {
  test.each(screens)('each %s strip button runs the action its key runs', (screen) => {
    for (const item of STRIPS[screen].filter((item) => !item.toggle))
      expect([item.keys, keyAction(screen, item.keys)]).toEqual([item.keys, item.action])
  })

  test('the range toggle stands for the Shift keys, which walk and extend', () => {
    expect(STRIPS.list.find((item) => item.toggle)).toMatchObject({ keys: '⇧j ⇧k', action: 'range' })
    expect(['⇧j', '⇧k', 'j', 'ArrowDown'].map((key) => keyAction('list', key))).toEqual([
      'next',
      'previous',
      'next',
      'next',
    ])
  })

  test('the list strip reads the picker keys the right-click menu shows', () => {
    for (const { kind, hint } of PICKERS) expect(keyAction('list', hint.length === 1 ? hint.toLowerCase() : hint)).toBe(kind)
  })

  test('keys mean their screen: l is labels on the list and a move on the board; g is gates only on the issue page', () => {
    expect([keyAction('list', 'l'), keyAction('board', 'l')]).toEqual(['labels', 'right'])
    expect([keyAction('issue', 'g'), keyAction('issue', 'g p'), keyAction('inbox', 'g p')]).toEqual([
      'gates',
      undefined,
      'pull-requests',
    ])
    expect([keyAction('inbox', 'Tab'), keyAction('list', 'Tab')]).toEqual(['next', undefined])
  })

  test('only the filter and the composer put the keyboard up', () => {
    const typing = screens.flatMap((screen) => STRIPS[screen].filter((item) => item.types).map((item) => item.what))
    expect(typing).toEqual(['filter', 'filter', 'comment'])
  })
})
