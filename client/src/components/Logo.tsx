// 与应用图标一致的矢量 Logo
export function Logo({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 1024 1024" aria-hidden>
      <defs>
        <linearGradient id="lg-bg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#7B5CFF" />
          <stop offset="0.52" stopColor="#4C7DFF" />
          <stop offset="1" stopColor="#12D3EE" />
        </linearGradient>
      </defs>
      <rect x="32" y="32" width="960" height="960" rx="224" fill="url(#lg-bg)" />
      <g fill="none" stroke="#fff" strokeLinecap="round" strokeWidth="54">
        <path d="M 330 409 A 154 154 0 0 1 484 255" />
        <path d="M 226 409 A 258 258 0 0 1 484 151" strokeOpacity="0.6" />
      </g>
      <path
        fill="#fff"
        stroke="#fff"
        strokeWidth="44"
        strokeLinejoin="round"
        d="M 484 409 L 484 783 L 583 700 L 645 846 L 708 820 L 645 674 L 770 674 Z"
      />
    </svg>
  );
}
