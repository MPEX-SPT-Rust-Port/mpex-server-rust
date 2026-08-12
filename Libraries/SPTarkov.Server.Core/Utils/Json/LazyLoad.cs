namespace SPTarkov.Server.Core.Utils.Json;

public class LazyLoad<T>(Func<T> deserialize)
{
    private readonly List<Func<T?, T?>> _lazyLoadTransformers = [];
    private readonly ReaderWriterLockSlim _lazyLoadTransformersLock = new();
    private readonly Func<ReadOnlyMemory<byte>?>? _readRawJson;

    /// <summary>
    /// A lazy load that can also hand out the raw JSON <see cref="Value"/> is built from, for callers
    /// that would only encode it straight back out again. <paramref name="readRawJson"/> must return
    /// the exact UTF-8 bytes <paramref name="deserialize"/> reads, or null when they are gone.
    /// </summary>
    public LazyLoad(Func<T> deserialize, Func<ReadOnlyMemory<byte>?> readRawJson)
        : this(deserialize)
    {
        _readRawJson = readRawJson;
    }

    /// <summary>
    /// Adds a transformer to modify the value during lazy loading. Transformers execute
    /// in registration order and the final result is cached until auto-cleanup.
    /// </summary>
    /// <param name="transformer">Function that transforms the value</param>
    public void AddTransformer(Func<T?, T?> transformer)
    {
        _lazyLoadTransformersLock.EnterWriteLock();

        try
        {
            _lazyLoadTransformers.Add(transformer);
        }
        finally
        {
            _lazyLoadTransformersLock.ExitWriteLock();
        }
    }

    /// <summary>
    /// Whether anything has registered a transformer. Only meaningful to a caller that wants
    /// <see cref="ReadRawJson"/>: transformers run on the deserialised object, so the raw JSON is
    /// equivalent to <see cref="Value"/> only while this is false.
    /// </summary>
    public bool HasTransformers
    {
        get
        {
            _lazyLoadTransformersLock.EnterReadLock();

            try
            {
                return _lazyLoadTransformers.Count > 0;
            }
            finally
            {
                _lazyLoadTransformersLock.ExitReadLock();
            }
        }
    }

    /// <summary>
    /// Whether this instance was given a raw JSON source, so <see cref="ReadRawJson"/> can return
    /// something. Cheap: it does not read anything.
    /// </summary>
    public bool HasRawJson
    {
        get { return _readRawJson is not null; }
    }

    /// <summary>
    /// The raw UTF-8 JSON <see cref="Value"/> deserialises, or null when this instance has no raw
    /// source or the source is gone. Re-reads on every call, exactly as <see cref="Value"/> does.
    /// Equivalent to <see cref="Value"/> only while <see cref="HasTransformers"/> is false.
    /// </summary>
    public ReadOnlyMemory<byte>? ReadRawJson()
    {
        return _readRawJson?.Invoke();
    }

    public T? Value
    {
        get
        {
            var result = deserialize();

            _lazyLoadTransformersLock.EnterReadLock();
            try
            {
                foreach (var transform in _lazyLoadTransformers)
                {
                    result = transform(result);
                }
            }
            finally
            {
                _lazyLoadTransformersLock.ExitReadLock();
            }

            return result;
        }
    }
}
