import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/ui',
  workers: 1,
  use: {
    baseURL: 'http://127.0.0.1:5190',
    viewport: { width: 393, height: 852 },
    launchOptions: { executablePath: process.env.FOLIO_BROWSER || 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe' },
  },
  webServer: { command: 'npm.cmd run dev -- --port 5190', url: 'http://127.0.0.1:5190', reuseExistingServer: true },
});
