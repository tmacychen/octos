interface Props {
  running: boolean
  className?: string
}

export default function StatusBadge({ running, className = '' }: Props) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 text-xs font-medium ${className}`}
    >
      <span
        className={`w-2 h-2 rounded-full ${
          running ? 'bg-green-400 shadow-[0_0_6px_rgba(74,222,128,0.6)]' : 'bg-gray-500'
        }`}
      />
      <span className={running ? 'text-green-400' : 'text-gray-500'}>
        {running ? 'Running' : 'Stopped'}
      </span>
    </span>
  )
}
