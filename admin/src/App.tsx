// App shell — wired in Phase 2 (routing + nav)
function App() {
  return (
    <div style={{ fontFamily: 'var(--font-sans)', color: 'var(--fg)', background: 'var(--bg)', minHeight: '100vh', display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
      <div style={{ textAlign: 'center' }}>
        <div style={{ fontFamily: 'var(--font-display)', fontWeight: 800, fontSize: 28, marginBottom: 8 }}>Stitchd Admin</div>
        <div style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Design system loaded — routing coming in Phase 2</div>
      </div>
    </div>
  )
}

export default App
