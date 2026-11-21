Draw.loadPlugin(function(ui)
{
	var nextId = 0;

	function sendResult(requestId, result)
	{
		if (window.opener != null && typeof window.opener.postMessage === 'function')
		{
			window.opener.postMessage({
				event: 'folio_tool_result',
				request_id: requestId,
				result: result
			});
		}
	}

	function cellSummary(cell)
	{
		var model = ui.editor.graph.getModel();
		var geometry = model.getGeometry(cell);
		var edge = model.isEdge(cell);

		return {
			id: cell.id,
			kind: edge ? 'edge' : (model.isVertex(cell) ? 'vertex' : 'cell'),
			value: cell.value == null ? '' : String(cell.value),
			style: cell.style || '',
			geometry: geometry == null ? null : {
				x: geometry.x,
				y: geometry.y,
				width: geometry.width,
				height: geometry.height,
				relative: geometry.relative
			},
			parent_id: cell.parent == null ? null : cell.parent.id,
			source_id: edge && cell.source != null ? cell.source.id : null,
			target_id: edge && cell.target != null ? cell.target.id : null
		};
	}

	function selectedCells()
	{
		var graph = ui.editor.graph;

		return {
			status: 'ok',
			cells: graph.getSelectionCells().map(function(cell)
			{
				return cellSummary(cell);
			})
		};
	}

	function cellIdFor(operation, model)
	{
		if (operation.id != null) return operation.id;

		var id;
		do
		{
			id = 'folio-' + Date.now() + '-' + (++nextId);
		}
		while (model.getCell(id) != null);

		return id;
	}

	function validateOperations(operations, model)
	{
		var added = new Map();
		var removed = new Set();
		var cellType = function(id)
		{
			if (removed.has(id)) return null;
			if (added.has(id)) return added.get(id);
			var cell = model.getCell(id);
			if (cell == null) return null;
			return model.isEdge(cell) ? 'edge' : (model.isVertex(cell) ? 'vertex' : 'cell');
		};

		for (var i = 0; i < operations.length; i++)
		{
			var operation = operations[i];

			if (operation.op == 'add_vertex' || operation.op == 'add_edge')
			{
				operation.id = cellIdFor(operation, model);

				if (model.getCell(operation.id) != null || added.has(operation.id))
				{
					throw new Error('新增 cell ID 已存在: ' + operation.id);
				}

				if (operation.parent_id != null && cellType(operation.parent_id) == null)
				{
					throw new Error('找不到父级 cell ID: ' + operation.parent_id);
				}

				if (operation.op == 'add_edge' &&
					(cellType(operation.source_id) != 'vertex' || cellType(operation.target_id) != 'vertex'))
				{
					throw new Error('连线的 source_id 和 target_id 必须指向已存在的顶点。');
				}

				added.set(operation.id, operation.op == 'add_edge' ? 'edge' : 'vertex');
			}
			else if (operation.op == 'update' || operation.op == 'delete')
			{
				var type = cellType(operation.id);
				if (type == null)
				{
					throw new Error('找不到 cell ID: ' + operation.id);
				}

				if (operation.op == 'update' && operation.geometry != null &&
					!added.has(operation.id) && model.getGeometry(model.getCell(operation.id)) == null)
				{
					throw new Error('cell 没有可编辑的几何信息: ' + operation.id);
				}

				if (operation.op == 'update' &&
					(operation.source_id != null || operation.target_id != null) && type != 'edge')
				{
					throw new Error('只有连线可以修改 source_id 或 target_id。');
				}

				if (operation.op == 'update' && operation.source_id != null &&
					cellType(operation.source_id) != 'vertex')
				{
					throw new Error('source_id 必须指向已存在的顶点。');
				}

				if (operation.op == 'update' && operation.target_id != null &&
					cellType(operation.target_id) != 'vertex')
				{
					throw new Error('target_id 必须指向已存在的顶点。');
				}

				if (operation.op == 'delete')
				{
					added.delete(operation.id);
					removed.add(operation.id);
				}
			}
		}
	}

	function applyOperations(operations)
	{
		var graph = ui.editor.graph;
		var model = graph.getModel();
		var defaultParent = graph.getDefaultParent();

		// Validate references before the transaction so rejected batches cannot partially alter the canvas.
		validateOperations(operations, model);
		model.beginUpdate();
		try
		{
			operations.forEach(function(operation)
			{
				var cell = model.getCell(operation.id);

				if (operation.op == 'add_vertex')
				{
					var parent = operation.parent_id == null ? defaultParent : model.getCell(operation.parent_id);
					graph.insertVertex(parent, operation.id, operation.value, operation.x,
						operation.y, operation.width, operation.height, operation.style || '');
				}
				else if (operation.op == 'add_edge')
				{
					var parent = operation.parent_id == null ? defaultParent : model.getCell(operation.parent_id);
					graph.insertEdge(parent, operation.id, operation.value,
						model.getCell(operation.source_id), model.getCell(operation.target_id),
						operation.style || '');
				}
				else if (operation.op == 'delete')
				{
					graph.removeCells([cell]);
				}
				else
				{
					if (Object.prototype.hasOwnProperty.call(operation, 'value'))
					{
						model.setValue(cell, operation.value);
					}
					if (Object.prototype.hasOwnProperty.call(operation, 'style'))
					{
						model.setStyle(cell, operation.style);
					}
					if (operation.geometry != null)
					{
						var geometry = model.getGeometry(cell).clone();
						Object.keys(operation.geometry).forEach(function(key)
						{
							geometry[key] = operation.geometry[key];
						});
						model.setGeometry(cell, geometry);
					}
					if (operation.source_id != null)
					{
						model.setTerminal(cell, model.getCell(operation.source_id), true);
					}
					if (operation.target_id != null)
					{
						model.setTerminal(cell, model.getCell(operation.target_id), false);
					}
				}
			});
		}
		finally
		{
			model.endUpdate();
		}

		return { status: 'ok', applied: operations.length };
	}

	window.__folioDrawioTool = function(raw)
	{
		var request;
		try
		{
			request = JSON.parse(raw);
			var result = request.operation == 'get_selection' ? selectedCells() :
				applyOperations(request.params.operations);
			sendResult(request.request_id, result);
		}
		catch (error)
		{
			sendResult(request == null ? null : request.request_id, {
				status: 'error',
				error: { code: 'drawio_operation_failed', message: error.message }
			});
		}
	};
});
