'use client'

import { useState, useEffect } from 'react'

export default function Home() {
  const [lastSynced, setLastSynced] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [fetchingStatus, setFetchingStatus] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [successMsg, setSuccessMsg] = useState<string | null>(null)

  const fetchStatus = async () => {
    try {
      const res = await fetch('/api/sync/status')
      if (res.ok) {
        const data = await res.json()
        if (data.last_synced) {
          setLastSynced(new Date(data.last_synced).toLocaleString())
        } else {
          setLastSynced('Never')
        }
      }
    } catch (err) {
      console.error('Failed to fetch status:', err)
    } finally {
      setFetchingStatus(false)
    }
  }

  useEffect(() => {
    fetchStatus()
  }, [])

  const handleSync = async () => {
    setLoading(true)
    setError(null)
    setSuccessMsg(null)

    try {
      const res = await fetch('/api/sync', { method: 'POST' })
      const data = await res.json()

      if (!res.ok) {
        throw new Error(data.message || 'Failed to sync')
      }

      setSuccessMsg(data.message)
      fetchStatus() // refresh timestamp
    } catch (err) {
      setError(err instanceof Error ? err.message : 'An error occurred during sync')
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-[#0a0f1c] text-slate-200">
      <div className="absolute inset-0 bg-[url('https://grainy-gradients.vercel.app/noise.svg')] opacity-20 pointer-events-none"></div>
      <div className="relative group max-w-lg w-full">
        {/* Glow effect behind */}
        <div className="absolute -inset-1 bg-gradient-to-r from-blue-600 to-cyan-500 rounded-2xl blur opacity-25 group-hover:opacity-40 transition duration-1000 group-hover:duration-200"></div>

        <div className="relative bg-[#111827] ring-1 ring-white/10 rounded-2xl shadow-2xl p-8 sm:p-12 overflow-hidden">
          <div className="relative z-10 flex flex-col items-center text-center">
            <div className="w-16 h-16 bg-blue-500/10 rounded-full flex items-center justify-center mb-6 ring-1 ring-blue-500/20 shadow-[0_0_15px_rgba(59,130,246,0.5)]">
              <svg
                xmlns="http://www.w3.org/polymorphic"
                className="w-8 h-8 text-blue-400"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={1.5}
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M8 7V3m8 4V3m-9 8h10M5 21h14a2 2 0 002-2V7a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z"
                />
              </svg>
            </div>

            <h1 className="text-3xl font-extrabold tracking-tight text-white mb-2">
              CalDAV{' '}
              <span className="text-transparent bg-clip-text bg-gradient-to-r from-blue-400 to-cyan-300">
                Sync
              </span>
            </h1>

            <p className="text-slate-400 mb-8 max-w-xs text-sm">
              Keep your CalDAV events perfectly synchronized into a unified ICS format.
            </p>

            <div className="w-full bg-slate-900/50 border border-slate-700/50 rounded-xl p-4 mb-8 flex justify-between items-center">
              <span className="text-sm font-medium text-slate-400">Last Synced</span>
              <span className="text-sm font-semibold text-slate-200">
                {fetchingStatus ? (
                  <span className="animate-pulse bg-slate-700 h-4 w-24 rounded inline-block"></span>
                ) : (
                  lastSynced
                )}
              </span>
            </div>

            <button
              onClick={handleSync}
              disabled={loading || fetchingStatus}
              className={`w-full group relative flex justify-center py-3.5 px-4 rounded-xl font-semibold transition-all duration-300 ${
                loading || fetchingStatus
                  ? 'bg-slate-800 text-slate-500 cursor-not-allowed'
                  : 'bg-white text-slate-900 hover:bg-slate-100 hover:scale-[1.02] shadow-[0_0_20px_rgba(255,255,255,0.1)]'
              }`}
            >
              {loading ? (
                <span className="flex items-center gap-2">
                  <svg
                    className="animate-spin -ml-1 mr-2 h-5 w-5 text-current"
                    xmlns="http://www.w3.org/2000/svg"
                    fill="none"
                    viewBox="0 0 24 24"
                  >
                    <circle
                      className="opacity-25"
                      cx="12"
                      cy="12"
                      r="10"
                      stroke="currentColor"
                      strokeWidth="4"
                    ></circle>
                    <path
                      className="opacity-75"
                      fill="currentColor"
                      d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                    ></path>
                  </svg>
                  Synchronizing...
                </span>
              ) : (
                'Sync Now'
              )}
            </button>

            {successMsg && (
              <div className="mt-6 w-full p-4 bg-emerald-500/10 border border-emerald-500/20 text-emerald-400 text-sm rounded-xl font-medium animate-in fade-in zoom-in duration-300">
                {successMsg}
              </div>
            )}

            {error && (
              <div className="mt-6 w-full p-4 bg-rose-500/10 border border-rose-500/20 text-rose-400 text-sm rounded-xl font-medium animate-in fade-in zoom-in duration-300">
                {error}
              </div>
            )}

            <div className="mt-6">
              <a
                href="/api/sync/ics"
                target="_blank"
                className="text-sm text-blue-400 hover:text-blue-300 transition-colors font-medium flex items-center gap-1 group"
              >
                Download ICS File
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  fill="none"
                  viewBox="0 0 24 24"
                  strokeWidth={2}
                  stroke="currentColor"
                  className="w-4 h-4 group-hover:translate-x-0.5 transition-transform"
                >
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    d="M13.5 4.5L21 12m0 0l-7.5 7.5M21 12H3"
                  />
                </svg>
              </a>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
