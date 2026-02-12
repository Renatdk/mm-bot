import './globals.css';
import type { Metadata } from 'next';
import Link from 'next/link';

export const metadata: Metadata = {
  title: 'MM Bot Control',
  description: 'Backtest orchestration dashboard'
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <header className="topbar">
          <div className="container row-between">
            <Link className="brand" href="/">
              MM Bot Control
            </Link>
            <a
              className="link"
              href={process.env.NEXT_PUBLIC_API_BASE_URL || '#'}
              target="_blank"
              rel="noreferrer"
            >
              API
            </a>
          </div>
        </header>
        <main className="container main">{children}</main>
      </body>
    </html>
  );
}
