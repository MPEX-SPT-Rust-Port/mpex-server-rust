using System.Collections;
using System.Collections.Frozen;
using System.Linq.Expressions;
using System.Reflection;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Exceptions.Database;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Utils.Json;

namespace SPTarkov.Server.Core.Utils;

[Injectable(InjectionType.Singleton)]
public sealed class ImporterUtil(ISptLogger<ImporterUtil> logger, FileUtil fileUtil, JsonUtil jsonUtil)
{
    // Skipped by the recursive database import only: locales load through their own services and the
    // suit/quest archives are not imported at all. The native verifier hashes all of them regardless.
    private readonly FrozenSet<string> _directoriesToIgnore = ["./SPT_Data/database/locales/server", "./SPT_Data/database/locales/web"];
    private readonly FrozenSet<string> _filesToIgnore = ["bearsuits.json", "usecsuits.json", "archivedquests.json"];

    public async Task<T> LoadRecursiveAsync<T>(
        string filePath,
        Func<string, CancellationToken, Task>? onReadCallback = null,
        Func<string, object, CancellationToken, Task>? onObjectDeserialized = null,
        CancellationToken cancellationToken = default
    )
    {
        var result = await LoadRecursiveAsync(filePath, typeof(T), null, onReadCallback, onObjectDeserialized, cancellationToken);

        return (T)result;
    }

    /// <summary>
    ///     Load files into objects recursively, reading from <paramref name="preloadedFiles"/> in place of
    ///     disk wherever the walk reaches a file the map holds.
    /// </summary>
    /// <param name="filePath">Path to folder with files</param>
    /// <param name="preloadedFiles">
    /// File bodies keyed manifest-style (<c>database/…</c>), or null to read everything from disk.
    /// </param>
    /// <param name="cancellationToken">
    /// The <see cref="CancellationToken"/> that can be used to cancel the loading operation.
    /// </param>
    /// <returns>Task</returns>
    internal async Task<T> LoadRecursiveAsync<T>(
        string filePath,
        IReadOnlyDictionary<string, ReadOnlyMemory<byte>>? preloadedFiles,
        CancellationToken cancellationToken = default
    )
    {
        var result = await LoadRecursiveAsync(filePath, typeof(T), preloadedFiles, null, null, cancellationToken);

        return (T)result;
    }

    /// <summary>
    ///     Load files into objects recursively (asynchronous)
    /// </summary>
    /// <param name="filePath">Path to folder with files</param>
    /// <param name="loadedType"></param>
    /// <param name="preloadedFiles"></param>
    /// <param name="onReadCallback"></param>
    /// <param name="onObjectDeserialized"></param>
    /// <param name="cancellationToken">
    /// The <see cref="CancellationToken"/> that can be used to cancel the loading operation.
    /// </param>
    /// <returns>Task</returns>
    private async Task<object> LoadRecursiveAsync(
        string filePath,
        Type loadedType,
        IReadOnlyDictionary<string, ReadOnlyMemory<byte>>? preloadedFiles,
        Func<string, CancellationToken, Task>? onReadCallback = null,
        Func<string, object, CancellationToken, Task>? onObjectDeserialized = null,
        CancellationToken cancellationToken = default
    )
    {
        cancellationToken.ThrowIfCancellationRequested();

        var tasks = new List<Task>();
        var dictionaryLock = new Lock();
        var result = Activator.CreateInstance(loadedType);

        // get all filepaths
        var files = fileUtil.GetFiles(filePath);
        var directories = fileUtil.GetDirectories(filePath);

        // Process files
        foreach (var file in files)
        {
            cancellationToken.ThrowIfCancellationRequested();

            if (
                fileUtil.GetFileExtension(file) != "json"
                || _filesToIgnore.Contains(fileUtil.GetFileNameAndExtension(file).ToLowerInvariant())
            )
            {
                continue;
            }

            tasks.Add(
                ProcessFileAsync(
                    file,
                    loadedType,
                    preloadedFiles,
                    onReadCallback,
                    onObjectDeserialized,
                    result,
                    dictionaryLock,
                    cancellationToken
                )
            );
        }

        // Process directories
        foreach (var directory in directories)
        {
            cancellationToken.ThrowIfCancellationRequested();

            if (_directoriesToIgnore.Contains(directory))
            {
                continue;
            }

            tasks.Add(
                ProcessDirectoryAsync(
                    directory,
                    loadedType,
                    result,
                    preloadedFiles,
                    onReadCallback,
                    onObjectDeserialized,
                    dictionaryLock,
                    cancellationToken
                )
            );
        }

        // Wait for all tasks to finish
        await Task.WhenAll(tasks);

        return result;
    }

    private async Task ProcessFileAsync(
        string file,
        Type loadedType,
        IReadOnlyDictionary<string, ReadOnlyMemory<byte>>? preloadedFiles,
        Func<string, CancellationToken, Task>? onReadCallback,
        Func<string, object, CancellationToken, Task>? onObjectDeserialized,
        object result,
        Lock dictionaryLock,
        CancellationToken cancellationToken = default
    )
    {
        cancellationToken.ThrowIfCancellationRequested();

        try
        {
            if (onReadCallback != null)
            {
                await onReadCallback(file, cancellationToken);
            }

            cancellationToken.ThrowIfCancellationRequested();

            // Get the set method to update the object
            var setMethod = GetSetMethod(
                fileUtil.StripExtension(file).ToLowerInvariant(),
                loadedType,
                out var propertyType,
                out var isDictionary
            );

            var fileDeserialized = await DeserializeFileAsync(file, propertyType, preloadedFiles, cancellationToken);

            if (onObjectDeserialized != null)
            {
                await onObjectDeserialized(file, fileDeserialized, cancellationToken);
            }

            lock (dictionaryLock)
            {
                setMethod.Invoke(result, isDictionary ? [fileUtil.StripExtension(file), fileDeserialized] : [fileDeserialized]);
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch (ValidationErrorException)
        {
            throw;
        }
        catch (Exception ex)
        {
            logger.Critical($"Unable to deserialize or find properties on file '{file}'", ex);
            throw new Exception($"Unable to deserialize or find properties on file '{file}'", ex);
        }
    }

    private async Task ProcessDirectoryAsync(
        string directory,
        Type loadedType,
        object result,
        IReadOnlyDictionary<string, ReadOnlyMemory<byte>>? preloadedFiles,
        Func<string, CancellationToken, Task>? onReadCallback,
        Func<string, object, CancellationToken, Task>? onObjectDeserialized,
        Lock dictionaryLock,
        CancellationToken cancellationToken = default
    )
    {
        cancellationToken.ThrowIfCancellationRequested();

        try
        {
            var directoryName = directory.Split("/").Last().Replace("_", "");

            if (MongoId.IsValidMongoId(directoryName))
            {
                // For trader MongoId directories, we need to get the parent property. Get parent directory name to find the property
                var parentDirectory = directory.Substring(0, directory.LastIndexOf('/'));
                var parentName = parentDirectory.Split("/").Last().Replace("_", "");

                GetSetMethod(parentName, loadedType, out var matchedProperty, out _);

                var loadedData = await LoadRecursiveAsync(
                    $"{directory}/",
                    matchedProperty,
                    preloadedFiles,
                    onReadCallback,
                    onObjectDeserialized,
                    cancellationToken
                );

                cancellationToken.ThrowIfCancellationRequested();

                lock (dictionaryLock)
                {
                    // Traders already have a dictionary, so we only need to handle this here
                    if (result is IDictionary dictionary)
                    {
                        dictionary[new MongoId(directoryName)] = loadedData;
                    }
                }
            }
            else
            {
                var setMethod = GetSetMethod(directoryName, loadedType, out var matchedProperty, out var isDictionary);

                var loadedData = await LoadRecursiveAsync(
                    $"{directory}/",
                    matchedProperty,
                    preloadedFiles,
                    onReadCallback,
                    onObjectDeserialized,
                    cancellationToken
                );

                lock (dictionaryLock)
                {
                    setMethod.Invoke(result, isDictionary ? [directory, loadedData] : [loadedData]);
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            throw new Exception($"Error processing directory '{directory}'", ex);
        }
    }

    private async Task<object> DeserializeFileAsync(
        string file,
        Type propertyType,
        IReadOnlyDictionary<string, ReadOnlyMemory<byte>>? preloadedFiles,
        CancellationToken cancellationToken = default
    )
    {
        if (propertyType.IsGenericType && propertyType.GetGenericTypeDefinition() == typeof(LazyLoad<>))
        {
            return CreateLazyLoadDeserialization(file, propertyType);
        }

        if (preloadedFiles is not null && preloadedFiles.TryGetValue(PreloadKey(file), out var preloaded))
        {
            return jsonUtil.Deserialize(preloaded.Span, propertyType)
                ?? throw new Exception($"Preloaded buffer for '{file}' deserialized to null");
        }

        // Absent from the map (lazy-adjacent files, or native/importer skip-list drift): disk, as always.
        return await jsonUtil.DeserializeFromFileAsync(file, propertyType, cancellationToken);
    }

    /// <summary>
    ///     "./SPT_Data/database/templates/items.json" -> "database/templates/items.json". Keys off the last
    ///     "database/" segment so test trees under any root resolve too.
    /// </summary>
    private static string PreloadKey(string file)
    {
        var normalized = file.Replace('\\', '/');
        var index = normalized.LastIndexOf("database/", StringComparison.Ordinal);

        return index < 0 ? normalized : normalized[index..];
    }

    private object CreateLazyLoadDeserialization(string file, Type propertyType)
    {
        var genericArgument = propertyType.GetGenericArguments()[0];

        var deserializeCall = Expression.Call(
            Expression.Constant(jsonUtil),
            "DeserializeFromFile",
            Type.EmptyTypes,
            Expression.Constant(file),
            Expression.Constant(genericArgument)
        );

        var typeAsExpression = Expression.TypeAs(deserializeCall, genericArgument);

        var expression = Expression.Lambda(typeof(Func<>).MakeGenericType(genericArgument), typeAsExpression);

        var expressionDelegate = expression.Compile();

        // The file the deserialisation reads, handed over unparsed for callers that only re-encode it
        Func<ReadOnlyMemory<byte>?> readRawJson = () =>
        {
            if (!File.Exists(file))
            {
                return null;
            }

            var bytes = File.ReadAllBytes(file);
            ReadOnlySpan<byte> utf8Bom = [0xEF, 0xBB, 0xBF]; // serde_json rejects it; System.Text.Json tolerates it
            return bytes.AsSpan().StartsWith(utf8Bom) ? bytes.AsMemory(utf8Bom.Length) : bytes;
        };

        return Activator.CreateInstance(propertyType, expressionDelegate, readRawJson);
    }

    public MethodInfo GetSetMethod(string propertyName, Type type, out Type propertyType, out bool isDictionary)
    {
        MethodInfo? setMethod;
        isDictionary = false;

        if (TryGetDictionaryValueType(type, out var dictionaryValueType))
        {
            propertyType = dictionaryValueType;
            setMethod = type.GetMethod("Add") ?? throw new Exception($"Unable to find Add method for dictionary type '{type.Name}'");
            isDictionary = true;
        }
        else
        {
            var strippedPropertyName = fileUtil.StripExtension(propertyName);

            var matchedProperty =
                type.GetProperties()
                    .FirstOrDefault(prop =>
                        string.Equals(prop.Name.ToLowerInvariant(), strippedPropertyName.ToLowerInvariant(), StringComparison.Ordinal)
                    )
                ?? throw new Exception($"Unable to find property '{strippedPropertyName}' for type '{type.Name}'");
            propertyType = matchedProperty.PropertyType;
            setMethod =
                matchedProperty.GetSetMethod()
                ?? throw new Exception($"Unable to find setter for property '{matchedProperty.Name}' on type '{type.Name}'");
        }

        return setMethod;
    }

    private static bool TryGetDictionaryValueType(Type type, out Type? valueType)
    {
        var currentType = type;

        while (currentType is not null)
        {
            if (currentType.IsGenericType)
            {
                if (currentType.GetGenericTypeDefinition() == typeof(Dictionary<,>))
                {
                    valueType = currentType.GetGenericArguments()[1];
                    return true;
                }
            }

            currentType = currentType.BaseType;
        }

        valueType = null;
        return false;
    }
}
