import '@testing-library/jest-dom/vitest'
import { afterEach, vi } from 'vitest'
import { cleanup } from '@testing-library/react'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

/**
 * Stubs de Tauri para que los módulos que importan `@tauri-apps/api/*`
 * no revienten al montar en jsdom. Los tests unitarios NO ejercitan
 * IPC; los tests que sí necesiten un cmd concreto lo mockean por
 * partida con `vi.mock(...)` en el propio fichero de test.
 */
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isFullscreen: vi.fn(async () => false),
    setFullscreen: vi.fn(async () => {}),
  }),
}))

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn(async () => () => {}),
  }),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}))
