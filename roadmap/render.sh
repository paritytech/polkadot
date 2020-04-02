# requires skill-tree: github.com/nikomatsakis/skill-tree

render () {
	echo "Rendering $1"
	skill-tree $1.toml output
	python3 -c "from graphviz import render; render('dot', 'svg', 'output/skill-tree.dot')"
	mv output/skill-tree.dot.svg "$1.svg"
	rm -rf output
}

render phase-1
