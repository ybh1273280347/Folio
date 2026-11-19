/** 临时 Python 片段复用 Pi 的命令生命周期和输出处理。 */
import { Type } from '@sinclair/typebox';

// Folio 锁定了 Pi 版本。复用 Bash 和 PowerShell 共用的执行器，只替换解释器与输入通道。
const { createLocalShellOperations, createShellToolDefinition } = await import(new URL(
  './core/tools/bash.js', import.meta.resolve('@earendil-works/pi-coding-agent'),
));

export function pythonTool(cwd) {
  const operations = createLocalShellOperations('Python', () => ({
    shell: 'python', args: ['-X', 'utf8', '-u', '-'], commandTransport: 'stdin',
  }));
  const tool = createShellToolDefinition(cwd, {
    name: 'run_python', label: 'Run Python', shellName: 'Python', tempFilePrefix: 'folio-python',
    promptSnippet: 'Run temporary Python source directly in the current Folio project',
  }, {
    operations,
    spawnHook: context => {
      // 进程继承本机 Python 环境，Folio 的网关配置只留在宿主内。
      delete context.env.FOLIO_AI_SETTINGS;
      return context;
    },
  });
  return {
    ...tool,
    description: 'Run a temporary, non-interactive Python snippet in the current project using python on PATH. Pass raw multiline Python as source. It is sent directly to Python over stdin without shell parsing or a script file. Use this tool for one-off Python work. Returns combined stdout and stderr and the exit code.',
    parameters: Type.Object({
      source: Type.String({ minLength: 1, description: 'Raw Python source code, without Markdown fences' }),
      timeout: Type.Optional(Type.Number({ exclusiveMinimum: 0, description: 'Timeout in seconds. Defaults to 30.' })),
    }, { additionalProperties: false }),
    execute: (id, { source, timeout = 30 }, signal, onUpdate, ctx) =>
      tool.execute(id, { command: source, timeout }, signal, onUpdate, ctx),
  };
}
