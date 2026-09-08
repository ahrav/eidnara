import { bundleModuleGraph } from "./module-graph";

/**
 * Bundles each entry named on the command line and prints one JSON object of
 * `{ inputs, externals }` per entry, keyed by entry path. The test runner
 * shares its module registry with the in-process bundler, so a test that
 * mocks a module would leak into a graph built in the same process; running
 * the bundler here, in a child process, keeps the graph honest.
 */
const graphs: Record<string, { inputs: string[]; externals: string[] }> = {};
for (const entry of process.argv.slice(2)) {
    const graph = await bundleModuleGraph(entry);
    graphs[entry] = { inputs: graph.inputs, externals: graph.externals };
}
process.stdout.write(JSON.stringify(graphs));
