import { describe, expect, it, vi } from 'vitest'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import {
  ErrorOverlay,
  PlayerLoader,
  SubDragErrorToast,
  SubDragFlash,
  SubDragOverlay,
  SyncHud,
} from './overlays'

/**
 * Los componentes de overlays reciben `useT` a través del hook —
 * `useT` está mockeado en `src/test/setup.ts` con un fallback que
 * devuelve la key literal cuando no hay diccionario. Aquí los tests
 * solo verifican el gate (visible / oculto) y la interacción del
 * botón, no el texto exacto.
 */

// Mock i18n para que t(key) devuelva la key literal — así podemos
// afirmar sobre las keys sin depender del diccionario real.
vi.mock('../../lib/i18n', () => ({
  useT: () => (k: string) => k,
}))

describe('SyncHud', () => {
  it('renders text when provided', () => {
    render(<SyncHud text="offset +0.5s" />)
    expect(screen.getByText('offset +0.5s')).toBeInTheDocument()
  })

  it('renders nothing when text is null', () => {
    const { container } = render(<SyncHud text={null} />)
    expect(container).toBeEmptyDOMElement()
  })
})

describe('ErrorOverlay', () => {
  it('shows message and Back button when error is set', () => {
    const onBack = vi.fn()
    render(<ErrorOverlay error="oh no" onBack={onBack} />)
    expect(screen.getByText('oh no')).toBeInTheDocument()
    // El label del botón es la key común.back (mock devuelve la key).
    expect(screen.getByRole('button', { name: 'common.back' })).toBeInTheDocument()
  })

  it('invokes onBack when button is clicked', async () => {
    const onBack = vi.fn()
    render(<ErrorOverlay error="oh no" onBack={onBack} />)
    await userEvent.click(screen.getByRole('button', { name: 'common.back' }))
    expect(onBack).toHaveBeenCalledOnce()
  })

  it('renders nothing when error is null', () => {
    const { container } = render(<ErrorOverlay error={null} onBack={vi.fn()} />)
    expect(container).toBeEmptyDOMElement()
  })
})

describe('SubDragOverlay', () => {
  it('renders drop hint when active', () => {
    render(<SubDragOverlay active={true} />)
    expect(screen.getByText('player.subDropTitle')).toBeInTheDocument()
    expect(screen.getByText('player.subDropHint')).toBeInTheDocument()
  })

  it('renders nothing when inactive', () => {
    const { container } = render(<SubDragOverlay active={false} />)
    expect(container).toBeEmptyDOMElement()
  })
})

describe('SubDragFlash', () => {
  it('renders a flash ring when active', () => {
    const { container } = render(<SubDragFlash active={true} />)
    // Un div con las clases de animación / ring.
    const el = container.querySelector('div')
    expect(el).not.toBeNull()
    expect(el?.className).toContain('animate-drop-flash')
  })

  it('renders nothing when inactive', () => {
    const { container } = render(<SubDragFlash active={false} />)
    expect(container).toBeEmptyDOMElement()
  })
})

describe('SubDragErrorToast', () => {
  it('shows message when present', () => {
    render(<SubDragErrorToast message="Invalid file" />)
    expect(screen.getByText('Invalid file')).toBeInTheDocument()
  })

  it('renders nothing when message is null', () => {
    const { container } = render(<SubDragErrorToast message={null} />)
    expect(container).toBeEmptyDOMElement()
  })
})

describe('PlayerLoader', () => {
  const base = {
    error: null,
    stream: null,
    hasStartedPlayback: false,
    seeking: false,
    audioSwitching: false,
    buffering: false,
    stalledLong: false,
    title: 'Blade Runner 2049',
    backdropUrl: null,
    logoUrl: null,
    stats: null,
  }

  it('renders nothing when error is set (delegated to ErrorOverlay)', () => {
    const { container } = render(<PlayerLoader {...base} error="boom" />)
    expect(container).toBeEmptyDOMElement()
  })

  it('renders full StremioLoader when stream is null (initial arrival)', () => {
    render(<PlayerLoader {...base} />)
    // El full loader pinta el título.
    expect(screen.getByText('Blade Runner 2049')).toBeInTheDocument()
  })

  it('renders full loader when audio is switching', () => {
    render(
      <PlayerLoader
        {...base}
        stream={{}}
        hasStartedPlayback={true}
        audioSwitching={true}
      />,
    )
    expect(screen.getByText('Blade Runner 2049')).toBeInTheDocument()
  })

  it('renders full loader when stalledLong', () => {
    render(
      <PlayerLoader
        {...base}
        stream={{}}
        hasStartedPlayback={true}
        buffering={true}
        stalledLong={true}
      />,
    )
    expect(screen.getByText('Blade Runner 2049')).toBeInTheDocument()
  })

  it('renders light spinner when seeking with a short stall', () => {
    const { container } = render(
      <PlayerLoader
        {...base}
        stream={{}}
        hasStartedPlayback={true}
        seeking={true}
        stalledLong={false}
      />,
    )
    // Light: no aparece el título, solo un div con clase animate-spin.
    expect(container.querySelector('.animate-spin')).not.toBeNull()
    expect(within(container).queryByText('Blade Runner 2049')).toBeNull()
  })

  it('renders nothing when everything is quiet', () => {
    const { container } = render(
      <PlayerLoader
        {...base}
        stream={{}}
        hasStartedPlayback={true}
        seeking={false}
        buffering={false}
        audioSwitching={false}
        stalledLong={false}
      />,
    )
    expect(container).toBeEmptyDOMElement()
  })
})
