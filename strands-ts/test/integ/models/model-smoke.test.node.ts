/**
 * Mantle rows authenticate with the ambient AWS credential chain, which only exists in node.
 */

import { describeSmokeModels, nodeOnlySmokeModels } from '../__fixtures__/model-smoke.js'

describeSmokeModels(nodeOnlySmokeModels)
